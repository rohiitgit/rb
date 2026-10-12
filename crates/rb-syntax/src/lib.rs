//! Syntax extraction (design §4.4): Tree-sitter definitions and references per file.
//!
//! [`extract`] is a pure function of `(language, bytes)`; together with [`EXTRACTOR_VERSION`] its
//! output can be cached by blob SHA. [`symbol_keys`] derives stable heuristic symbol keys (design
//! §5.4) from a file's path and facts alone.

mod bindings;
mod calls;
mod decorations;
mod imports;
mod keys;
mod mentions;
mod queries;
mod strings;
mod tokens;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Parser, QueryCursor, StreamingIterator};

pub use keys::{symbol_keys, type_symbol_key};
pub use tokens::surface_names;

/// Grammar versions (checked against Cargo.lock by a test). Query text and the extractor's own
/// source code are fingerprinted automatically by [`extractor_id`], so no manual bump is needed.
pub const EXTRACTOR_VERSION: &str = "rb-syntax;tree-sitter-0.27.0;typescript-0.23.2;javascript-0.25.0;java-0.23.5;python-0.25.0;go-0.25.0;kotlin-ng-1.1.0;csharp-0.23.5";

/// The cache identity of this extractor: [`EXTRACTOR_VERSION`] plus a fingerprint of every
/// bundled query and of the extraction source code. Any edit to either invalidates cached facts,
/// so a stale cache can never be served (a manual version bump was forgotten twice).
pub fn extractor_id() -> &'static str {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| {
        format!(
            "{EXTRACTOR_VERSION};code-{:016x}",
            fnv1a(fingerprint_input().as_bytes())
        )
    })
}

/// Everything that determines `extract`'s output besides the grammars.
fn fingerprint_input() -> String {
    [
        queries::all_sources().as_str(),
        include_str!("lib.rs"),
        include_str!("queries.rs"),
        include_str!("strings.rs"),
        include_str!("decorations.rs"),
        include_str!("imports.rs"),
        include_str!("bindings.rs"),
        include_str!("calls.rs"),
        include_str!("tokens.rs"),
        include_str!("mentions.rs"),
    ]
    .join("\n")
}

/// 64-bit FNV-1a: tiny, stable across platforms and releases, good enough for a fingerprint.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Definitions nested deeper than this are dropped (reported via `nesting_truncated`). Keys embed
/// their ancestors, so unbounded depth would make key size quadratic in file size.
pub const MAX_NESTING: usize = 64;

/// Files larger than this are not parsed (design §4.1).
pub const MAX_PARSE_BYTES: u64 = 1024 * 1024;

pub type Span = rb_model::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Language {
    TypeScript,
    Tsx,
    JavaScript,
    Java,
    Python,
    Go,
    Kotlin,
    CSharp,
}

impl Language {
    pub fn as_str(self) -> &'static str {
        match self {
            Language::TypeScript => "typescript",
            Language::Tsx => "tsx",
            Language::JavaScript => "javascript",
            Language::Java => "java",
            Language::Python => "python",
            Language::Go => "go",
            Language::Kotlin => "kotlin",
            Language::CSharp => "csharp",
        }
    }

    /// Maps `FileEntry::lang` (from `rb_git::language_for_path`) to a parser, if one exists.
    pub fn from_lang(lang: &str) -> Option<Self> {
        Some(match lang {
            "typescript" => Language::TypeScript,
            "tsx" => Language::Tsx,
            "javascript" => Language::JavaScript,
            "java" => Language::Java,
            "python" => Language::Python,
            "go" => Language::Go,
            "kotlin" => Language::Kotlin,
            "csharp" => Language::CSharp,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DefKind {
    Module,
    Class,
    Interface,
    Type,
    Function,
    Method,
    Constant,
}

impl DefKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DefKind::Module => "module",
            DefKind::Class => "class",
            DefKind::Interface => "interface",
            DefKind::Type => "type",
            DefKind::Function => "function",
            DefKind::Method => "method",
            DefKind::Constant => "constant",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RefKind {
    Call,
    Class,
    Type,
    Implementation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Definition {
    pub kind: DefKind,
    pub name: String,
    pub name_span: Span,
    /// Span of the whole definition node.
    pub span: Span,
    /// Index into `FileFacts::definitions` of the innermost enclosing definition.
    pub parent: Option<usize>,
    /// Go method receiver type, when the type is not defined in the same file (else `parent`).
    pub receiver: Option<String>,
    /// Java annotations, Python/TypeScript decorators, in source order.
    #[serde(default)]
    pub decorations: Vec<Decoration>,
    /// Classes and interfaces this definition extends or implements, in source order.
    #[serde(default)]
    pub supertypes: Vec<Supertype>,
    /// Python class bodies: `NAME = 'string'` assignments (e.g. `__tablename__`), in order.
    #[serde(default)]
    pub class_constants: Vec<ClassConstant>,
    /// Declared type parameters (`BaseRepo<T, ID>` → `T`, `ID`), for Java and TypeScript.
    #[serde(default)]
    pub type_params: Vec<String>,
    /// The declared return type as written, type arguments included: a Java method's (`void`
    /// too; not a constructor's), a TypeScript method's or function's annotation (contract M4b
    /// S1).
    #[serde(default)]
    pub return_type: Option<String>,
    /// Modifiers as written, sorted: Java `public`, `protected`, `private`, `static`,
    /// `abstract`, `final`, `default`; TS/JS `export`, `export_default`, `abstract`, `static`.
    #[serde(default)]
    pub modifiers: Vec<String>,
}

/// An annotation or decorator on a definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decoration {
    /// As written, possibly qualified (`Table`, `app.route`, `javax.persistence.Entity`).
    pub name: String,
    pub args: Vec<DecorationArg>,
    /// The whole annotation or decorator (`@PostMapping("/login")`), so evidence can point at it.
    #[serde(default)]
    pub span: Span,
}

/// One decoration argument. String literals (and `+` concatenations of them) are decoded, with
/// interpolations as `$n`; anything else is its source text. An object literal argument
/// (`@Entity({ name: 'x' })`) contributes its pairs as keyed arguments.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DecorationArg {
    pub key: Option<String>,
    pub value: String,
    /// `value` is a fixed string known statically (no identifiers, calls or interpolations).
    #[serde(default)]
    pub literal: bool,
    /// The elements of an array value (Java `{"/a", "/b"}`, a TS or Python list), each decoded
    /// like a value; empty for any other value.
    #[serde(default)]
    pub items: Vec<DecorationArg>,
}

/// A named supertype; `name` is as written (possibly qualified), type arguments by simple name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Supertype {
    pub name: String,
    pub type_args: Vec<String>,
    /// An interface the class implements (Java `implements`, TypeScript `implements`), as
    /// opposed to the class it extends.
    #[serde(default)]
    pub implements: bool,
    /// Span of the type's name only, the qualified name as written (`Foo`, `a.b.Outer.Inner`),
    /// without type arguments and without type-use annotations (contract M3b amendment 43), so
    /// an exact index's type occurrence inside it names this type and not an argument's.
    pub span: Span,
}

/// A name assigned in a Python class body. Strings are decoded and booleans kept as
/// `True`/`False`; any other value is its source text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassConstant {
    pub name: String,
    pub value: String,
    /// `value` is a static string or boolean (not an f-string, a name or a call).
    #[serde(default)]
    pub literal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub kind: RefKind,
    pub name: String,
    pub span: Span,
    /// Index into `FileFacts::definitions` of the innermost definition containing the reference.
    pub enclosing: Option<usize>,
    /// For a member call, the object it is called on as written (`prisma.user` in
    /// `prisma.user.findMany()`); for a qualified type, its package or outer type (`store` in Go
    /// `store.DB`). `None` for plain calls and simple type names.
    #[serde(default)]
    pub qualifier: Option<String>,
}

/// A string expression that looks like SQL (design §7.1). The SQL parser decides downstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqlCandidate {
    /// Normalised text: escapes decoded, interpolations and non-literal operands as `$n`.
    pub text: String,
    /// Span of the whole string expression.
    pub span: Span,
    /// Index into `FileFacts::definitions` of the innermost definition containing it.
    pub enclosing: Option<usize>,
    /// Name of the function it is a direct argument of (or that tags it, for templates).
    pub callee: Option<String>,
}

/// A string expression as literal parts and holes (contract M4a S1): a literal, a template
/// string, an f-string or a `+` concatenation with at least one literal. Literal parts are
/// decoded as for SQL candidates; every interpolation or non-literal operand is a hole that keeps
/// its expression text (at most [`MAX_EXPR_BYTES`]) and span. Adjacent literal parts are merged.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StrTemplate {
    pub parts: Vec<TemplatePart>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TemplatePart {
    Lit { text: String },
    Hole { expr: String, span: Span },
}

/// Expression texts kept in facts (holes, names, receiver roots) are cut to this many bytes.
pub const MAX_EXPR_BYTES: usize = 256;

/// An outermost call expression and the calls chained onto its receiver (contract M4a S1):
/// `router.route('/x').get(h).post(h2)` is the root `router` and the links `route`, `get`,
/// `post`, in source order. Calls nested in arguments or callbacks are chains of their own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallChain {
    /// The receiver of the first link as written (`app`, `this.restTemplate`, `axios`, `http`),
    /// `None` when the chain starts with a plain call (`fetch(…)`, `express()`).
    pub root: Option<String>,
    pub links: Vec<CallLink>,
    /// The whole outermost call expression.
    pub span: Span,
    /// Index into `FileFacts::definitions` of the innermost definition containing the call.
    pub enclosing: Option<usize>,
    /// The target as written when the call is the right side of an assignment (`this.rt`,
    /// `web`), so a pack can see what an instance is configured with in a constructor (contract
    /// M4a amendment 12). Declarations are bindings with an initialiser instead.
    #[serde(default)]
    pub assigned: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallLink {
    /// The called name (`post`, `getForObject`, `Group`); empty when the callee is not a name
    /// (`f()(x)`).
    pub name: String,
    /// Span of the name (of the callee when there is no name).
    pub span: Span,
    pub args: Vec<Arg>,
}

/// A summary of one call argument (contract M4a S1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Arg {
    /// A string expression.
    Template { template: StrTemplate },
    /// An identifier or a member path as written (`login`, `ctrl.login`, `HttpMethod.POST`).
    Name { text: String, span: Span },
    /// An inline function (arrow, function expression, lambda, Go func literal).
    Function { span: Span },
    /// An object literal, one level deep (nested objects are `Other`). A spread (`...cfg`) is
    /// an entry with the key `...`, in source order.
    Object {
        span: Span,
        entries: Vec<ObjectEntry>,
    },
    /// A Java object creation (`new DefaultUriBuilderFactory("http://x")`): the type as written
    /// and its arguments, one level deep (nested ones are `Other`).
    New {
        type_name: String,
        span: Span,
        args: Vec<Arg>,
    },
    /// A class literal (`Service.class`): the type as written, for `bind(X).to(Y)` shapes
    /// (contract M6j S2). Kotlin `X::class` and C# `typeof(X)` stay `Other`.
    Type { text: String, span: Span },
    /// Anything else (a call, a number, an array, …).
    Other { span: Span },
}

impl Arg {
    pub fn span(&self) -> Span {
        match self {
            Arg::Template { template } => template.span,
            Arg::Name { span, .. }
            | Arg::Function { span }
            | Arg::Object { span, .. }
            | Arg::New { span, .. }
            | Arg::Type { span, .. }
            | Arg::Other { span } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectEntry {
    /// The key as written, string keys decoded (`baseURL`, `method`).
    pub key: String,
    pub value: Arg,
}

/// A binding's initialiser (contract M4a S2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Init {
    Template {
        template: StrTemplate,
    },
    /// A call: index into `FileFacts::chains` of the call chain it is.
    Call {
        chain: usize,
    },
    /// A Java object creation that is not a call chain's root (`new RestTemplate()`): the type
    /// as written, without type arguments, and its number of arguments (contract M4a amendment
    /// 15: a client built by a bare constructor is a carrier Gate A can verify).
    New {
        type_name: String,
        args: usize,
        /// A class body follows (an anonymous subclass, which may override anything; contract
        /// M4a amendment 16).
        #[serde(default)]
        body: bool,
    },
    Name {
        text: String,
        span: Span,
    },
    /// A member of a destructured object (contract M6b S9): `const { get } =
    /// axios` holds `axios.get` under the local name. The object as written
    /// (a name, a member path, or a `require("module")` module); the member is
    /// the binding's own name. Never folded (a hole, like no initialiser).
    Member {
        object: String,
        span: Span,
    },
    /// `a ?? 'lit'` or `a || 'lit'`: the name `a` with a default string.
    Fallback {
        primary: String,
        default: StrTemplate,
    },
    /// A TS/JS object literal, one level deep as [`Arg::Object`] (contract M5 S19 (f): a spread
    /// naming the binding is read for the `baseURL` it may set).
    Object {
        entries: Vec<ObjectEntry>,
    },
}

/// `true` for `serde(default)` on facts that predated the field, all of
/// which were `this` writes.
fn default_true() -> bool {
    true
}

/// A member write `this.x = <value>` (TS/JS) or `this.x = <value>` (Java), in source
/// order (contract M6b S1). The value is summarised as an initialiser when it has a
/// foldable shape; `None` for anything else (still a write for single-assignment
/// counting in phase 6). `enclosing` is the innermost definition (a constructor check
/// reads its kind); writes in anonymous scopes resolve through `nearest_kept` like
/// every other fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberWrite {
    pub member: String,
    pub value: Option<Init>,
    /// The written object is `this` (contract M6b S6: anything else still
    /// counts for single-assignment, but never folds).
    #[serde(default = "default_true")]
    pub on_this: bool,
    pub enclosing: Option<usize>,
    /// The write sits in a constructor: a TS/JS `method_definition` named `constructor`
    /// or a Java `constructor_declaration` (TypeScript constructors are not definitions:
    /// their parameters sit in the class, so the kind cannot be read back later).
    #[serde(default)]
    pub ctor: bool,
    pub span: Span,
}

/// Everything extracted from one blob. Definitions are sorted by position (a Go method's parent
/// may be a type declared later in the file).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FileFacts {
    pub definitions: Vec<Definition>,
    pub references: Vec<Reference>,
    /// Span of the first syntax error, if the file did not parse cleanly.
    pub syntax_error: Option<Span>,
    /// Definitions deeper than [`MAX_NESTING`] were dropped.
    #[serde(default)]
    pub nesting_truncated: bool,
    /// SQL-looking string expressions, in source order.
    #[serde(default)]
    pub sql: Vec<SqlCandidate>,
    /// Import statements, in source order.
    #[serde(default)]
    pub imports: Vec<Import>,
    /// The Java `package` or Go `package` clause.
    #[serde(default)]
    pub package: Option<String>,
    /// Names bound to a declared or constructed type, in source order.
    #[serde(default)]
    pub bindings: Vec<Binding>,
    /// Outermost call expressions with their chained calls, in source order.
    #[serde(default)]
    pub chains: Vec<CallChain>,
    /// Member writes `this.x = <value>` (TS/JS, Java), in source order (contract M6b S1).
    #[serde(default)]
    pub member_writes: Vec<MemberWrite>,
    /// Every token of a name that may hold a configured object, in source order (contract M4a
    /// amendment 14).
    #[serde(default)]
    pub name_tokens: Vec<NameToken>,
    /// TS/JS: the local name a module's default export names (`export default x`), or the
    /// class or function `export default class X`/`function f` declares (contract M4b S1).
    #[serde(default)]
    pub default_export: Option<String>,
    /// TS/JS: the span of a default-exported call that is not a plain name
    /// (`export default Router().use(…)`; contract M6b S3). The route pack matches it
    /// against a call chain to bind anonymous default-exported routers.
    #[serde(default)]
    pub default_export_expr: Option<Span>,
    /// TS/JS: the module specifiers whose whole module object the file takes, in source order:
    /// `import * as m`, `export * from`, `export * as m from`, `require(…)` and `import(…)`;
    /// `*` for a `require`/`import` whose argument is not a plain string (evaluator pass 1, Y7).
    #[serde(default)]
    pub module_objects: Vec<String>,
    /// Java and TS/JS: every identifier, type identifier and property-name token's text,
    /// mapped to the roles it occurs in in this file, plus every whole word of a string
    /// literal (contract M4b S1). No spans.
    #[serde(default)]
    pub mentions: BTreeMap<String, BTreeSet<MentionRole>>,
}

/// The role of one mention of a name (contract M4b S1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MentionRole {
    /// The name of a definition being declared (class, interface, enum, record, constructor,
    /// method, function, type alias).
    Declaration,
    /// An import or unaliased export specifier, an import's local name, or a package clause.
    /// The source name of an aliased specifier (`{ T as U }`) is [`MentionRole::Other`].
    Import,
    /// The head of an `extends`/`implements` item (its qualifier included).
    Supertype,
    /// Any other type position, type arguments included.
    Type,
    /// The type of an object creation or anonymous class (Java), a `new` expression's
    /// constructor (TS/JS; for `new m.T()`, `T`).
    New,
    /// Java `X.class` (not inside `getLogger(…)`).
    ClassLiteral,
    /// Java `X::m`, `X::new`.
    MethodRef,
    /// The qualifier of a member access or static call, `X.y`, `X.m()`.
    Member,
    /// The name of an annotation or decorator (the decorator's callee, qualifiers included).
    Annotation,
    /// Java: a class literal that is the only argument of a `getLogger(…)` call.
    Logger,
    /// A whole word inside a string literal.
    String,
    /// Anything else: a call's name, a variable, field or parameter name, an argument, an
    /// export of a value, an aliased specifier's source name.
    Other,
}

/// One token of a name that may hold a configured object (contract M4a amendments 14–16): a
/// name a binding initialises with a call, or the simple target of an assignment from a call
/// (TypeScript, JavaScript, Java); a module object (a TS/JS default or namespace import of a
/// package); or a configuration-surface name (`fetch`; Go's `DefaultTransport`,
/// `DefaultClient`; Java's `RequestInterceptor`), also as a property, field or type name; or a
/// TS/JS string literal naming `fetch` or the axios module ([`TokenRole::Literal`], amendment
/// 17). Every token of such a name in the file is recorded, wherever it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameToken {
    pub name: String,
    pub span: Span,
    pub role: TokenRole,
}

/// What a [`NameToken`] is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TokenRole {
    /// The name of a TS/JS `const`/`let` declarator that is not exported (`export const` is
    /// [`TokenRole::Other`]), or of a Java local variable declarator.
    Declaration,
    /// TS/JS: the callee of a call, `name(…)`, or of a member call when the token is the
    /// member's name (`x.name(…)`; contract M4a amendment 16).
    Callee,
    /// The receiver of a member call, `name.call(…)`, without optional chaining.
    Receiver { call: String },
    /// TS/JS and Go: the object of a member access that is not called, `name.m` read or
    /// written, also continued (`name.m.n = …`, `name.m.f(…)`), without optional chaining
    /// (contract M4a amendment 15).
    Member { name: String },
    /// Anything else: an export, a Java member read or write, optional chaining, an argument,
    /// an assignment target, a parameter, a field, a `var` or another declaration.
    Other,
    /// TS/JS: a string literal whose content is the token's name, `fetch`, `axios` or an
    /// `axios/` subpath, wherever it is (an import or `require` source, `jest.spyOn(global,
    /// "fetch")`, `globalThis["fetch"]`); never one naming an ambient module declaration
    /// (contract M4a amendment 17).
    Literal,
}

/// One import statement (or one spec of a Go import block).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Import {
    /// The module as written: `./auth` or `lib` (TS/JS); the package `com.acme`, or for a static
    /// import the class `com.acme.B` (Java); `a.b`, `.` or `..p.q` (Python); `x/y` (Go).
    pub module: String,
    /// Names imported from the module, with their local names. TS/JS default imports are named
    /// `default`.
    pub names: Vec<ImportedName>,
    /// Local name bound to the module itself: TS/JS `* as m`, Python `import a.b as c`, Go
    /// `f "x/y"` (also `_` and `.`).
    pub alias: Option<String>,
    /// Java `.*`, Python `from m import *`.
    pub wildcard: bool,
    /// Java `import static`.
    pub is_static: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedName {
    pub name: String,
    pub local: String,
}

/// A name whose type is known from its declaration or its constructor: a field, parameter or
/// variable. `this.x`/`self.x` members are recorded by their member name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub name: String,
    /// The type as written, without type arguments or pointer/reference markers (`User`,
    /// `models.User`); never a built-in scalar type.
    pub type_name: String,
    /// Index into `FileFacts::definitions` of the scope: the enclosing function for locals and
    /// parameters, the enclosing class for members; `None` for top-level code.
    pub scope: Option<usize>,
    /// A field or member (`this.x`, `self.x`, a TS parameter property, a Go struct field).
    pub member: bool,
    pub span: Span,
    /// Where the declaration is in effect: from the end of the declaration to the end of its
    /// scope (a block, function or loop; the whole function in Python). A reference to the name
    /// outside this extent means another declaration.
    #[serde(default)]
    pub visible: Span,
    /// The initialiser, when it is a string, a call, a name, a `??`/`||` fallback or an object
    /// literal.
    #[serde(default)]
    pub init: Option<Init>,
    /// Declared constant: TS/JS `const`, a Java `final` variable or interface field, Go `const`.
    #[serde(default)]
    pub constant: bool,
    /// Java annotations on a field or parameter (`@Value("${k:d}")`), in source order.
    #[serde(default)]
    pub decorations: Vec<Decoration>,
    /// The 0-based position of a parameter in its function, method, constructor or lambda
    /// (Java, TS/JS, Go, Python; Python's `self` counts); `None` for fields, locals, a Go
    /// receiver and named results (contract M4b S1). A destructured parameter's names share it.
    #[serde(default)]
    pub parameter: Option<u32>,
    /// The declared type's arguments as simple names, one level deep (`List<Notifier>` →
    /// `Notifier`, `Map<String, a.Sink>` → `String`, `Sink`; a wildcard as its bound), Java and
    /// TypeScript; for an array ([`Binding::array`]) its element type alone.
    #[serde(default)]
    pub type_args: Vec<String>,
    /// The declared type is an array (`T[]`, Java `T...`), whose element is `type_args[0]`;
    /// `type_name` is then empty, as before.
    #[serde(default)]
    pub array: bool,
    /// A type argument, or an array's element, has type arguments of its own
    /// (`List<Store<String>>`), which [`Binding::type_args`] does not keep (evaluator pass 1,
    /// Y5).
    #[serde(default)]
    pub nested_type_args: bool,
    /// Java: the declaration's modifiers as written, sorted (as [`Definition::modifiers`]:
    /// `final`, `static`, `private`, …), for fields, locals and parameters (contract M4b
    /// amendment 6).
    #[serde(default)]
    pub modifiers: Vec<String>,
    /// Java: a field or local declarator has an initialiser (`= …`), whatever its kind.
    #[serde(default)]
    pub initialised: bool,
}

/// Parses `source` and extracts its definitions and references.
pub fn extract(language: Language, source: &[u8]) -> anyhow::Result<FileFacts> {
    let grammar = queries::for_language(language);
    let mut parser = Parser::new();
    parser.set_language(&grammar.language)?;
    let tree = parser
        .parse(source, None)
        .context("tree-sitter produced no tree")?;
    let root = tree.root_node();

    let (mut raw_defs, mut raw_refs) = capture(grammar, root, source);
    raw_defs.sort_by_key(|d| (d.node.start_byte(), std::cmp::Reverse(d.node.end_byte())));
    let raw_defs = drop_locals(raw_defs, grammar.anonymous_scopes);
    raw_refs.sort_by_key(|r| (r.name.start_byte(), r.kind));

    let lexical_parent = nesting(&raw_defs);
    let depth = depths(&lexical_parent);
    let kept: Vec<Option<usize>> = kept_indices(&depth);
    let nesting_truncated = kept.iter().any(Option::is_none);
    let mut definitions: Vec<Definition> = raw_defs
        .iter()
        .zip(&lexical_parent)
        .zip(&kept)
        .filter(|(_, new)| new.is_some())
        .map(|((d, parent), _)| Definition {
            kind: d.kind,
            name: text(d.name, source),
            name_span: span(d.name),
            span: span(d.node),
            // A kept definition's parent is shallower, so it is kept too.
            parent: parent.and_then(|p| kept[p]),
            receiver: (language == Language::Go)
                .then(|| go_receiver(d.node, source))
                .flatten(),
            decorations: decorations::decorations(language, d.node, source),
            supertypes: decorations::supertypes(language, d.node, source),
            class_constants: decorations::class_constants(language, d.node, source),
            type_params: decorations::type_params(language, d.node, source),
            return_type: decorations::return_type(language, d.node, source),
            modifiers: decorations::modifiers(language, d.node, source),
        })
        .collect();
    attach_go_methods(&mut definitions);
    for i in 0..definitions.len() {
        let parent_is_class = definitions[i]
            .parent
            .is_some_and(|p| definitions[p].kind == DefKind::Class);
        if definitions[i].kind == DefKind::Function && parent_is_class {
            definitions[i].kind = DefKind::Method;
        }
    }

    let starts: Vec<usize> = raw_defs.iter().map(|d| d.node.start_byte()).collect();
    let ends: Vec<usize> = raw_defs.iter().map(|d| d.node.end_byte()).collect();
    let references = raw_refs
        .iter()
        .map(|r| Reference {
            kind: r.kind,
            name: text(r.name, source),
            span: span(r.name),
            enclosing: enclosing(r.name.start_byte(), &starts, &ends, &lexical_parent)
                .map(|i| nearest_kept(i, &kept, &lexical_parent)),
            qualifier: match r.kind {
                RefKind::Call => call_qualifier(r.name, source),
                RefKind::Class | RefKind::Type => type_qualifier(r.name, source),
                RefKind::Implementation => None,
            },
        })
        .collect();

    let raw_chains = calls::chains(language, root, source);
    let chain_index: HashMap<usize, usize> = raw_chains
        .iter()
        .enumerate()
        .map(|(i, c)| (c.node.id(), i))
        .collect();
    let bindings: Vec<Binding> = bindings::bindings(language, root, source)
        .into_iter()
        .map(|b| {
            let mut scope = enclosing(b.node.start_byte(), &starts, &ends, &lexical_parent)
                .map(|i| nearest_kept(i, &kept, &lexical_parent));
            // Members belong to their class, even when assigned in a method.
            if b.member {
                while let Some(i) = scope.filter(|&i| {
                    !matches!(
                        definitions[i].kind,
                        DefKind::Class | DefKind::Interface | DefKind::Type
                    )
                }) {
                    scope = definitions[i].parent;
                }
            }
            let from = if b.from_start {
                b.from.start_position()
            } else {
                b.from.end_position()
            };
            let to = b.to.end_position();
            Binding {
                name: b.name,
                type_name: b.type_name,
                scope,
                member: b.member,
                span: span(b.node),
                visible: [
                    point(from.row),
                    point(from.column),
                    point(to.row),
                    point(to.column),
                ],
                init: b
                    .destructured_from
                    .clone()
                    .map(|object| Init::Member {
                        object,
                        span: span(b.node),
                    })
                    .or_else(|| {
                        b.value
                            .and_then(|v| calls::init(language, v, source, &chain_index))
                    }),
                constant: b.constant,
                // Contract M6j S1: Kotlin and C# bindings carry their annotations like
                // Java's do (definition decorations ride the same arms).
                decorations: if matches!(
                    language,
                    Language::Java | Language::Kotlin | Language::CSharp
                ) {
                    decorations::decorations(language, b.node, source)
                } else {
                    Vec::new()
                },
                parameter: b.parameter.map(|p| u32::try_from(p).unwrap_or(u32::MAX)),
                type_args: b.type_args,
                array: b.array,
                nested_type_args: b.nested_type_args,
                modifiers: b.modifiers,
                initialised: b.initialised,
            }
        })
        .collect();
    let chains: Vec<CallChain> = raw_chains
        .into_iter()
        .map(|c| CallChain {
            root: c.root,
            links: c.links,
            span: span(c.node),
            enclosing: enclosing(c.node.start_byte(), &starts, &ends, &lexical_parent)
                .map(|i| nearest_kept(i, &kept, &lexical_parent)),
            assigned: c.assigned,
        })
        .collect();
    let imports = imports::imports(language, root, source);
    let declared: HashSet<std::ops::Range<usize>> =
        raw_defs.iter().map(|d| d.name.byte_range()).collect();
    let defined: HashSet<std::ops::Range<usize>> = raw_defs
        .iter()
        .zip(&kept)
        .filter(|(_, k)| k.is_some())
        .map(|(d, _)| d.name.byte_range())
        .collect();
    let mentions = mentions::mentions(language, root, source, &declared, &defined);
    let name_tokens = tokens::tokens(
        language,
        root,
        source,
        &tokens::candidates(language, &bindings, &chains, &imports),
    );

    Ok(FileFacts {
        definitions,
        references,
        syntax_error: root.has_error().then(|| first_error(root)).flatten(),
        nesting_truncated,
        sql: strings::sql_candidates(language, root, source)
            .into_iter()
            .map(|c| SqlCandidate {
                span: span(c.node),
                enclosing: enclosing(c.node.start_byte(), &starts, &ends, &lexical_parent)
                    .map(|i| nearest_kept(i, &kept, &lexical_parent)),
                callee: c.callee,
                text: c.text,
            })
            .collect(),
        imports,
        package: imports::package(language, root, source),
        bindings,
        chains,
        member_writes: bindings::member_writes(language, root, source)
            .into_iter()
            .map(|w| MemberWrite {
                member: w.member,
                value: w
                    .value
                    .and_then(|v| calls::init(language, v, source, &chain_index)),
                on_this: w.on_this,
                enclosing: enclosing(w.node.start_byte(), &starts, &ends, &lexical_parent)
                    .map(|i| nearest_kept(i, &kept, &lexical_parent)),
                ctor: w.ctor,
                span: span(w.node),
            })
            .collect(),
        name_tokens,
        default_export: imports::default_export(language, root, source),
        default_export_expr: imports::default_export_expr(language, root, source),
        module_objects: imports::module_objects(language, root, source),
        mentions,
    })
}

/// Removes definitions that live inside an anonymous function (callbacks, lambdas, anonymous
/// classes, func literals) or inside another local: they are locals, not symbols (design §8.2).
/// A function value that *is* a captured definition (`const f = () => {}`) is not anonymous.
/// `defs` must be sorted outer-first, so each definition's ancestors are decided before it.
fn drop_locals<'t>(defs: Vec<RawDef<'t>>, anonymous_scopes: &[&str]) -> Vec<RawDef<'t>> {
    let mut is_local: HashMap<usize, bool> = HashMap::with_capacity(defs.len());
    let mut kept = Vec::with_capacity(defs.len());
    for d in defs {
        let local = local_by_ancestry(d.node, &is_local, anonymous_scopes);
        is_local.insert(d.node.id(), local);
        if !local {
            kept.push(d);
        }
    }
    kept
}

fn local_by_ancestry(
    node: Node<'_>,
    is_local: &HashMap<usize, bool>,
    anonymous_scopes: &[&str],
) -> bool {
    let mut cur = node.parent();
    while let Some(n) = cur {
        if let Some(&local) = is_local.get(&n.id()) {
            return local;
        }
        let names_a_definition = n.parent().is_some_and(|p| is_local.contains_key(&p.id()));
        if anonymous_scopes.contains(&n.kind()) && !names_a_definition {
            return true;
        }
        cur = n.parent();
    }
    false
}

/// Nesting depth of each definition (parents precede children in `parent`'s order).
fn depths(parent: &[Option<usize>]) -> Vec<usize> {
    let mut depth = vec![0; parent.len()];
    for i in 0..parent.len() {
        depth[i] = parent[i].map_or(0, |p| depth[p] + 1);
    }
    depth
}

/// New index of each definition within `MAX_NESTING`, `None` for dropped ones.
fn kept_indices(depth: &[usize]) -> Vec<Option<usize>> {
    let mut next = 0;
    depth
        .iter()
        .map(|&d| {
            (d < MAX_NESTING).then(|| {
                next += 1;
                next - 1
            })
        })
        .collect()
}

/// The new index of `i`, or of its closest kept ancestor (depth 0 is always kept).
fn nearest_kept(mut i: usize, kept: &[Option<usize>], parent: &[Option<usize>]) -> usize {
    loop {
        match (kept[i], parent[i]) {
            (Some(new), _) => return new,
            (None, Some(p)) => i = p,
            (None, None) => unreachable!("top-level definitions are always kept"),
        }
    }
}

struct RawDef<'t> {
    kind: DefKind,
    node: Node<'t>,
    name: Node<'t>,
}

struct RawRef<'t> {
    kind: RefKind,
    name: Node<'t>,
}

/// Runs the tag query; de-duplicates nodes captured by more than one pattern (first one wins).
fn capture<'t>(
    grammar: &queries::Grammar,
    root: Node<'t>,
    source: &[u8],
) -> (Vec<RawDef<'t>>, Vec<RawRef<'t>>) {
    let names = grammar.query.capture_names();
    let mut defs = Vec::new();
    let mut refs = Vec::new();
    let mut seen_defs = HashSet::new();
    let mut seen_refs = HashSet::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&grammar.query, root, source);
    while let Some(m) = matches.next() {
        let mut name = None;
        let mut role = None;
        for c in m.captures() {
            let capture = names[c.index as usize];
            if capture == "name" {
                name = Some(c.node);
            } else if let Some(kind) = capture.strip_prefix("definition.").and_then(def_kind) {
                role = Some((Some(kind), None, c.node));
            } else if let Some(kind) = capture.strip_prefix("reference.").and_then(ref_kind) {
                role = Some((None, Some(kind), c.node));
            }
        }
        let (Some(name), Some(role)) = (name, role) else {
            continue;
        };
        match role {
            (Some(kind), _, node) => {
                if seen_defs.insert((node.byte_range(), name.byte_range())) {
                    defs.push(RawDef { kind, node, name });
                }
            }
            (None, Some(kind), _) => {
                if seen_refs.insert((name.byte_range(), kind)) {
                    refs.push(RawRef { kind, name });
                }
            }
            (None, None, _) => {}
        }
    }
    (defs, refs)
}

fn def_kind(tag: &str) -> Option<DefKind> {
    Some(match tag {
        "module" => DefKind::Module,
        "class" => DefKind::Class,
        "interface" => DefKind::Interface,
        "type" => DefKind::Type,
        "function" => DefKind::Function,
        "method" => DefKind::Method,
        "constant" => DefKind::Constant,
        _ => return None,
    })
}

fn ref_kind(tag: &str) -> Option<RefKind> {
    Some(match tag {
        "call" => RefKind::Call,
        "class" => RefKind::Class,
        "type" => RefKind::Type,
        "implementation" => RefKind::Implementation,
        _ => return None,
    })
}

/// Innermost strictly-enclosing definition of each definition. `defs` is sorted by start
/// ascending, end descending, so a stack sweep suffices.
fn nesting(defs: &[RawDef<'_>]) -> Vec<Option<usize>> {
    let mut stack: Vec<usize> = Vec::new();
    let mut parents = Vec::with_capacity(defs.len());
    for (i, d) in defs.iter().enumerate() {
        while stack
            .last()
            .is_some_and(|&top| defs[top].node.end_byte() < d.node.end_byte())
        {
            stack.pop();
        }
        parents.push(stack.last().copied());
        stack.push(i);
    }
    parents
}

/// Innermost definition whose node contains `byte`: the last definition starting at or before
/// it, then up the lexical parent chain until one also ends after it.
fn enclosing(
    byte: usize,
    starts: &[usize],
    ends: &[usize],
    parent: &[Option<usize>],
) -> Option<usize> {
    let mut candidate = starts.partition_point(|&s| s <= byte).checked_sub(1);
    while let Some(i) = candidate {
        if ends[i] > byte {
            return Some(i);
        }
        candidate = parent[i];
    }
    None
}

/// The object a member call is made on: the `object` (JS/TS, Python, Java) or `operand` (Go) of
/// the node whose name the call reference captured.
/// The package or outer type of a qualified type reference: `store` in Go `store.DB`, `a.b` in
/// Java `a.b.C`, `ns` in TS `ns.T`.
fn type_qualifier(name: Node<'_>, source: &[u8]) -> Option<String> {
    let parent = name.parent()?;
    let scope = match parent.kind() {
        "qualified_type" => parent.child_by_field_name("package"),
        "nested_type_identifier" => parent.child_by_field_name("module"),
        "scoped_type_identifier" => parent.named_child(0),
        "member_expression" => parent.child_by_field_name("object"),
        "attribute" => parent.child_by_field_name("object"),
        _ => None,
    }
    .filter(|s| s.id() != name.id())?;
    Some(text(scope, source))
}

fn call_qualifier(name: Node<'_>, source: &[u8]) -> Option<String> {
    let member = name.parent()?;
    // Kotlin navigation receivers are positional (no `object:` field): the receiver precedes
    // the member name (contract M6i S2).
    if member.kind() == "navigation_expression" {
        let mut c = member.walk();
        let receiver = member.named_children(&mut c).next()?;
        if receiver.id() == name.id() {
            return None;
        }
        return Some(text(receiver, source));
    }
    let object = member
        .child_by_field_name("object")
        .or_else(|| member.child_by_field_name("operand"))
        .or_else(|| member.child_by_field_name("expression"))
        .filter(|o| o.id() != name.id())?;
    Some(text(object, source))
}

/// Go: the receiver's type name of a `method_declaration`, e.g. `Circle` for `(c *Circle)`.
fn go_receiver(node: Node<'_>, source: &[u8]) -> Option<String> {
    let receiver = node.child_by_field_name("receiver")?;
    let mut cursor = receiver.walk();
    let mut stack = vec![receiver];
    while let Some(n) = stack.pop() {
        if n.kind() == "type_identifier" {
            return Some(text(n, source));
        }
        let children: Vec<Node<'_>> = n.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    None
}

/// Go methods become children of their receiver type when it is declared at the top level of the
/// same file. Receivers always name package-level types, so types local to a function (including
/// the method's own body) never qualify; that also keeps the parent relation acyclic.
fn attach_go_methods(defs: &mut [Definition]) {
    let types: HashMap<String, usize> = defs
        .iter()
        .enumerate()
        .filter(|(_, d)| d.kind == DefKind::Type && d.parent.is_none())
        .rev() // so the first declaration wins
        .map(|(i, d)| (d.name.clone(), i))
        .collect();
    for d in defs.iter_mut() {
        if d.parent.is_none()
            && let Some(&t) = d.receiver.as_ref().and_then(|r| types.get(r))
        {
            d.parent = Some(t);
            d.receiver = None;
        }
    }
}

/// First `ERROR` or `MISSING` node in document order.
fn first_error(root: Node<'_>) -> Option<Span> {
    let mut cursor = root.walk();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.is_error() || n.is_missing() {
            return Some(span(n));
        }
        if n.has_error() {
            let children: Vec<Node<'_>> = n.children(&mut cursor).collect();
            stack.extend(children.into_iter().rev());
        }
    }
    None
}

fn text(node: Node<'_>, source: &[u8]) -> String {
    String::from_utf8_lossy(&source[node.byte_range()]).into_owned()
}

pub(crate) fn span(node: Node<'_>) -> Span {
    let (s, e) = (node.start_position(), node.end_position());
    [point(s.row), point(s.column), point(e.row), point(e.column)]
}

pub(crate) fn point(v: usize) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod type_qualifier_tests {
    use crate::{Language, RefKind, extract};

    fn qualified(language: Language, src: &str) -> Vec<(String, Option<String>)> {
        extract(language, src.as_bytes())
            .unwrap()
            .references
            .into_iter()
            .filter(|r| matches!(r.kind, RefKind::Type | RefKind::Class))
            .map(|r| (r.name, r.qualifier))
            .collect()
    }

    #[test]
    fn qualified_type_references_keep_their_qualifier() {
        let go = qualified(
            Language::Go,
            "package p\ntype S struct { db store.DB; n Local }\n",
        );
        assert!(go.contains(&("DB".into(), Some("store".into()))), "{go:?}");
        assert!(go.contains(&("Local".into(), None)), "{go:?}");
        // Java's tags capture simple type names only (`new Y()`), never scoped ones.
        let java = qualified(
            Language::Java,
            "class K { void m() { new Y(); new x.Z(); } }\n",
        );
        assert_eq!(java, [("Y".to_owned(), None)]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `kind qualified.name@row:col` per definition, where the qualifier is the parent chain (or
    /// the Go receiver).
    fn outline(language: Language, src: &str) -> Vec<String> {
        let facts = extract(language, src.as_bytes()).unwrap();
        let qualified = |i: usize| {
            let mut parts = vec![facts.definitions[i].name.clone()];
            let mut cur = facts.definitions[i].parent;
            while let Some(p) = cur {
                parts.push(facts.definitions[p].name.clone());
                cur = facts.definitions[p].parent;
            }
            if let Some(r) = &facts.definitions[i].receiver {
                parts.push(r.clone());
            }
            parts.reverse();
            parts.join(".")
        };
        facts
            .definitions
            .iter()
            .enumerate()
            .map(|(i, d)| {
                format!(
                    "{} {}@{}:{}",
                    d.kind.as_str(),
                    qualified(i),
                    d.name_span[0],
                    d.name_span[1]
                )
            })
            .collect()
    }

    /// `kind name in enclosing-name` for each reference of the given kind.
    fn refs(language: Language, src: &str, kind: RefKind) -> Vec<String> {
        let facts = extract(language, src.as_bytes()).unwrap();
        facts
            .references
            .iter()
            .filter(|r| r.kind == kind)
            .map(|r| {
                let enclosing = r
                    .enclosing
                    .map_or("-".to_owned(), |i| facts.definitions[i].name.clone());
                format!("{} in {enclosing}", r.name)
            })
            .collect()
    }

    const JAVA: &str = "package com.acme;

public class AuthController {
    private final Mfa mfa;
    public AuthController(Mfa mfa) { this.mfa = mfa; }
    public String login(String e) { return mfa.send(e); }
    public String login(String e, String p) { return login(e); }
    public record LoginRequest(String email) {}
    enum Mode { A, B }
}
interface Mfa { String send(String e); }
";

    #[test]
    fn java_definitions_including_records_enums_and_constructors() {
        assert_eq!(
            outline(Language::Java, JAVA),
            [
                "class AuthController@2:13",
                "method AuthController.AuthController@4:11",
                "method AuthController.login@5:18",
                "method AuthController.login@6:18",
                "class AuthController.LoginRequest@7:18",
                "class AuthController.Mode@8:9",
                "interface Mfa@10:10",
                "method Mfa.send@10:23",
            ]
        );
        assert_eq!(
            refs(Language::Java, JAVA, RefKind::Call),
            ["send in login", "login in login"]
        );
    }

    const KOTLIN: &str = "package acme

interface Greeter {
    fun greet(name: String): String
}

class DefaultGreeter : Greeter {
    override fun greet(name: String): String {
        return Helper.shout(name)
    }
}

fun main() {
    val g = DefaultGreeter()
    g.greet(\"hi\")
}
";

    /// Contract M6i S1: Kotlin definitions pin classes, interfaces, methods and top-level
    /// functions with hand-written name spans; calls pin bare and member callees.
    #[test]
    fn kotlin_definitions_and_calls() {
        assert_eq!(
            outline(Language::Kotlin, KOTLIN),
            [
                "interface Greeter@2:10",
                "method Greeter.greet@3:8",
                "class DefaultGreeter@6:6",
                "method DefaultGreeter.greet@7:17",
                "function main@12:4",
            ]
        );
        assert_eq!(
            refs(Language::Kotlin, KOTLIN, RefKind::Call),
            ["shout in greet", "DefaultGreeter in main", "greet in main"]
        );
    }

    const CSHARP: &str = "using System;
namespace Acme {
    public interface IGreeter { string Greet(string name); }
    public class Greeter : IGreeter {
        public string Greet(string name) { return Helper.Shout(name); }
    }
    public static class Program {
        public static void Main() { new Greeter().Greet(\"x\"); }
    }
}
";

    /// Contract M6i S1: C# definitions pin namespaces, interfaces, classes and methods with
    /// hand-written name spans; invocations pin member callees.
    #[test]
    fn csharp_definitions_and_calls() {
        assert_eq!(
            outline(Language::CSharp, CSHARP),
            [
                "module Acme@1:10",
                "interface Acme.IGreeter@2:21",
                "method Acme.IGreeter.Greet@2:39",
                "class Acme.Greeter@3:17",
                "method Acme.Greeter.Greet@4:22",
                "class Acme.Program@6:24",
                "method Acme.Program.Main@7:27",
            ]
        );
        assert_eq!(
            refs(Language::CSharp, CSHARP, RefKind::Call),
            ["Shout in Greet", "Greet in Main"]
        );
    }

    /// A record's compact canonical constructor is a constructor definition (named like the
    /// record) and the source of the calls in its body (contract M3b amendment 75).
    #[test]
    fn java_compact_constructors_are_constructors() {
        let src = "record Rec(int x) {\n  Rec(String s) { this(s.length()); }\n  Rec {\n    check(x);\n  }\n  static void check(int v) {}\n}\n";
        assert_eq!(
            outline(Language::Java, src),
            [
                "class Rec@0:7",
                "method Rec.Rec@1:2",
                "method Rec.Rec@2:2",
                "method Rec.check@5:14",
            ]
        );
        let facts = extract(Language::Java, src.as_bytes()).unwrap();
        let check = facts.references.iter().find(|r| r.name == "check").unwrap();
        let source = &facts.definitions[check.enclosing.unwrap()];
        assert_eq!(
            (source.kind, source.name_span),
            (DefKind::Method, [2, 2, 2, 5])
        );
    }

    const PYTHON: &str = "import os

class Auth:
    def login(self, email):
        return check(email)

    class Inner:
        pass

def check(email):
    def helper(x):
        return x
    return helper(email)

MAX = 3
";

    #[test]
    fn python_methods_nested_functions_and_constants() {
        assert_eq!(
            outline(Language::Python, PYTHON),
            [
                "class Auth@2:6",
                "method Auth.login@3:8",
                "class Auth.Inner@6:10",
                "function check@9:4",
                "function check.helper@10:8",
                "constant MAX@14:0",
            ]
        );
        assert_eq!(
            refs(Language::Python, PYTHON, RefKind::Call),
            ["check in login", "helper in check"]
        );
    }

    const GO: &str = "package shapes

type Circle struct{ R float64 }
type Square struct{ S float64 }

func (c *Circle) Area() float64 { return c.R * c.R * 3 }
func (s Square) Area() float64 { return s.S * s.S }

func Total(xs []Circle) float64 {
\tt := 0.0
\tfor _, x := range xs {
\t\tt += x.Area()
\t}
\treturn t
}

func (p *Polygon) Sides() int { return 0 }
";

    #[test]
    fn go_methods_attach_to_their_receiver_type() {
        assert_eq!(
            outline(Language::Go, GO),
            [
                "type Circle@2:5",
                "type Square@3:5",
                "method Circle.Area@5:17",
                "method Square.Area@6:16",
                "function Total@8:5",
                // Receiver type not defined in this file: kept as `receiver`, no parent.
                "method Polygon.Sides@16:18",
            ]
        );
        let facts = extract(Language::Go, GO.as_bytes()).unwrap();
        // Same-file receiver types become the lexical parent (so CONTAINS can be drawn).
        let circle = facts.definitions.iter().position(|d| d.name == "Circle");
        for area in facts.definitions.iter().filter(|d| d.name == "Area") {
            assert!(area.parent.is_some() && area.receiver.is_none(), "{area:?}");
        }
        assert_eq!(facts.definitions[2].parent, circle);
        let sides = facts
            .definitions
            .iter()
            .find(|d| d.name == "Sides")
            .unwrap();
        assert_eq!(
            (sides.parent, sides.receiver.as_deref()),
            (None, Some("Polygon"))
        );
        assert_eq!(refs(Language::Go, GO, RefKind::Call), ["Area in Total"]);
    }

    const TS: &str = "export interface Creds { email: string }
export abstract class Base { abstract run(): void; }
export class AuthService extends Base {
  run(): void {}
  login(c: Creds): string { return hash(c.email); }
}
export function hash(s: string): string {
  const inner = (x: string) => x;
  return inner(s);
}
";

    #[test]
    fn go_methods_only_attach_to_top_level_types() {
        // The receiver type is declared inside the method itself: must not become its parent.
        let own_body = "package p\nfunc (t T) M() {\n\ttype T struct{}\n}\n";
        let facts = extract(Language::Go, own_body.as_bytes()).unwrap();
        let m = facts.definitions.iter().find(|d| d.name == "M").unwrap();
        assert_eq!((m.parent, m.receiver.as_deref()), (None, Some("T")));

        // A same-named type local to another function is not the receiver either.
        let other_fn = "package p\nfunc f() {\n\ttype T struct{}\n}\nfunc (t T) M() {}\n";
        assert_eq!(
            outline(Language::Go, other_fn),
            ["function f@1:5", "type f.T@2:6", "method T.M@4:11"]
        );
    }

    #[test]
    fn nesting_is_capped_so_cost_stays_linear() {
        let depth = MAX_NESTING + 40;
        let src = format!("{}{}\n", "function a(){".repeat(depth), "}".repeat(depth));
        let facts = extract(Language::JavaScript, src.as_bytes()).unwrap();
        assert_eq!(facts.definitions.len(), MAX_NESTING);
        assert!(facts.nesting_truncated);
        assert!(
            facts
                .definitions
                .iter()
                .all(|d| d.parent.is_none_or(|p| p < facts.definitions.len()))
        );
        let shallow = extract(Language::JavaScript, b"function a(){ function b(){} }\n").unwrap();
        assert!(!shallow.nesting_truncated);
    }

    #[test]
    fn typescript_type_aliases_and_enums_are_definitions() {
        let src = "export type Id = string\nenum Color { Red }\nexport const enum Dir { Up }\n";
        assert_eq!(
            outline(Language::TypeScript, src),
            ["type Id@0:12", "class Color@1:5", "class Dir@2:18"]
        );
    }

    #[test]
    fn definitions_inside_anonymous_functions_are_locals() {
        // Test-style callbacks: the aliases are locals, not symbols (design §8.2).
        let ts = "describe('a', () => {\n  type Expected = 1\n  function helper() { go() }\n})\ndescribe('b', () => { type Expected = 2 })\nconst f = () => { type T = 1 }\n";
        assert_eq!(
            outline(Language::TypeScript, ts),
            ["function f@5:6", "type f.T@5:23"]
        );
        // A call inside a local definition still counts, attributed to no named definition.
        assert_eq!(
            refs(Language::TypeScript, ts, RefKind::Call),
            ["describe in -", "go in -", "describe in -"]
        );

        let java = "class A {\n  void m() {\n    Runnable r = new Runnable() { public void run() { go(); } };\n  }\n}\n";
        assert_eq!(
            outline(Language::Java, java),
            ["class A@0:6", "method A.m@1:7"]
        );
        assert_eq!(refs(Language::Java, java, RefKind::Call), ["go in m"]);

        let go = "package p\nfunc F() {\n\tg := func() { type L struct{} }\n\ttype K struct{}\n\t_ = g\n}\n";
        assert_eq!(
            outline(Language::Go, go),
            ["function F@1:5", "type F.K@3:6"]
        );
    }

    #[test]
    fn typescript_namespaces_are_modules() {
        let src =
            "namespace N { export function a() {} }\nnamespace M { export function a() {} }\n";
        assert_eq!(
            outline(Language::TypeScript, src),
            [
                "module N@0:10",
                "function N.a@0:30",
                "module M@1:10",
                "function M.a@1:30"
            ]
        );
    }

    #[test]
    fn typescript_uses_javascript_and_typescript_tags() {
        assert_eq!(
            outline(Language::TypeScript, TS),
            [
                "interface Creds@0:17",
                "class Base@1:22",
                "method Base.run@1:38",
                "class AuthService@2:13",
                "method AuthService.run@3:2",
                "method AuthService.login@4:2",
                "function hash@6:16",
                "function hash.inner@7:8",
            ]
        );
        assert_eq!(
            refs(Language::TypeScript, TS, RefKind::Call),
            ["hash in login", "inner in hash"]
        );
    }

    #[test]
    fn tsx_and_javascript_grammars() {
        let tsx = "export function App() { return <div onClick={() => go()} />; }\n";
        assert_eq!(outline(Language::Tsx, tsx), ["function App@0:16"]);
        assert_eq!(refs(Language::Tsx, tsx, RefKind::Call), ["go in App"]);

        let js = "class K { m() { return new K(); } }\n";
        assert_eq!(
            outline(Language::JavaScript, js),
            ["class K@0:6", "method K.m@0:10"]
        );
        assert_eq!(refs(Language::JavaScript, js, RefKind::Class), ["K in m"]);
    }

    #[test]
    fn member_calls_record_their_qualifier() {
        let qualifiers = |language, src: &str| -> Vec<(String, Option<String>)> {
            extract(language, src.as_bytes())
                .unwrap()
                .references
                .iter()
                .filter(|r| r.kind == RefKind::Call)
                .map(|r| (r.name.clone(), r.qualifier.clone()))
                .collect()
        };
        let q =
            |name: &str, qualifier: Option<&str>| (name.to_owned(), qualifier.map(str::to_owned));
        assert_eq!(
            qualifiers(
                Language::TypeScript,
                "async function f() { await this.prisma.user.findMany(); go(); }"
            ),
            [q("findMany", Some("this.prisma.user")), q("go", None)]
        );
        assert_eq!(
            qualifiers(Language::Python, "def f():\n    db.session.query(User)\n"),
            [q("query", Some("db.session"))]
        );
        assert_eq!(
            qualifiers(Language::Java, "class A { void f() { repo.findById(1); } }"),
            [q("findById", Some("repo"))]
        );
        assert_eq!(
            qualifiers(Language::Go, "package p\nfunc f() { db.Query(q) }\n"),
            [q("Query", Some("db"))]
        );
        assert_eq!(
            qualifiers(Language::Kotlin, "fun f() {\nHelper.shout(x)\n}\n"),
            [q("shout", Some("Helper"))]
        );
        assert_eq!(
            qualifiers(
                Language::CSharp,
                "class K {\nvoid M() {\nUtil.Shout(x);\n}\n}\n"
            ),
            [q("Shout", Some("Util"))]
        );
    }

    #[test]
    fn syntax_errors_keep_the_well_formed_parts() {
        let src = "def ok():\n    pass\n\ndef broken(:\n";
        let facts = extract(Language::Python, src.as_bytes()).unwrap();
        assert!(facts.definitions.iter().any(|d| d.name == "ok"));
        let err = facts.syntax_error.expect("syntax error reported");
        assert_eq!(err[0], 3, "first error on line 3, got {err:?}");
        assert_eq!(
            extract(Language::Python, b"def ok():\n    pass\n")
                .unwrap()
                .syntax_error,
            None
        );
    }

    #[test]
    fn spans_are_byte_columns_with_crlf_and_non_ascii() {
        let src = "def café():\r\n    pass\r\ndef b():\r\n    pass\r\n";
        let facts = extract(Language::Python, src.as_bytes()).unwrap();
        let spans: Vec<(String, Span)> = facts
            .definitions
            .iter()
            .map(|d| (d.name.clone(), d.name_span))
            .collect();
        assert_eq!(
            spans,
            [
                ("café".to_owned(), [0, 4, 0, 9]),
                ("b".to_owned(), [2, 4, 2, 5])
            ]
        );
    }

    #[test]
    fn facts_round_trip_through_json() {
        let facts = extract(Language::Java, JAVA.as_bytes()).unwrap();
        assert!(!facts.definitions.is_empty());
        let json = serde_json::to_vec(&facts).unwrap();
        assert_eq!(serde_json::from_slice::<FileFacts>(&json).unwrap(), facts);
    }

    #[test]
    fn language_mapping() {
        for (lang, expected) in [
            ("typescript", Some(Language::TypeScript)),
            ("tsx", Some(Language::Tsx)),
            ("javascript", Some(Language::JavaScript)),
            ("sql", None),
        ] {
            assert_eq!(Language::from_lang(lang), expected, "{lang}");
            if let Some(l) = expected {
                assert_eq!(l.as_str(), lang);
            }
        }
    }

    #[test]
    fn extractor_id_fingerprints_queries_and_code() {
        let id = extractor_id();
        assert!(id.starts_with(EXTRACTOR_VERSION), "{id}");
        let (_, hash) = id.rsplit_once(";code-").expect("code fingerprint present");
        assert_eq!(hash.len(), 16);
        assert_eq!(
            hash,
            format!("{:016x}", fnv1a(fingerprint_input().as_bytes()))
        );
        let input = fingerprint_input();
        assert!(
            input.contains("fn drop_locals"),
            "extractor code is fingerprinted"
        );
        assert!(
            input.contains("JAVA_SUPPLEMENT"),
            "query supplements are fingerprinted"
        );
        // FNV-1a reference value and sensitivity.
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_ne!(fnv1a(b"(a) @name"), fnv1a(b"(b) @name"));
    }

    /// Contract M4a S2: every source file of this crate that `extract` depends on is part of
    /// the fingerprint, so a new extractor module can never serve stale cached facts.
    /// `keys.rs` derives keys from facts after extraction and is the only exemption.
    #[test]
    fn every_extractor_module_is_fingerprinted() {
        const NOT_USED_BY_EXTRACT: &[&str] = &["keys.rs"];
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let input = fingerprint_input();
        let mut checked = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            if NOT_USED_BY_EXTRACT.contains(&name.as_str()) {
                let module = name.trim_end_matches(".rs");
                let lib = include_str!("lib.rs");
                let body =
                    &lib[lib.find("pub fn extract(").unwrap()..lib.find("fn drop_locals").unwrap()];
                assert!(
                    !body.contains(&format!("{module}::")),
                    "extract uses {name}"
                );
                continue;
            }
            let content = std::fs::read_to_string(&path).unwrap();
            assert!(
                input.contains(&content),
                "{name} is not in the extractor fingerprint"
            );
            checked.push(name);
        }
        checked.sort();
        assert!(checked.contains(&"calls.rs".to_owned()), "{checked:?}");
    }

    /// EXTRACTOR_VERSION must change whenever a grammar version changes.
    #[test]
    fn extractor_version_matches_cargo_lock() {
        let lock =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"))
                .unwrap();
        let version_of = |name: &str| {
            let marker = format!("name = \"{name}\"\nversion = \"");
            let start = lock
                .find(&marker)
                .unwrap_or_else(|| panic!("{name} not in Cargo.lock"))
                + marker.len();
            lock[start..].split('"').next().unwrap().to_owned()
        };
        for (crate_name, tag) in [
            ("tree-sitter", "tree-sitter"),
            ("tree-sitter-typescript", "typescript"),
            ("tree-sitter-javascript", "javascript"),
            ("tree-sitter-java", "java"),
            ("tree-sitter-python", "python"),
            ("tree-sitter-go", "go"),
            ("tree-sitter-kotlin-ng", "kotlin-ng"),
            ("tree-sitter-c-sharp", "csharp"),
        ] {
            let pinned = format!(";{tag}-{};", version_of(crate_name));
            assert!(
                format!("{EXTRACTOR_VERSION};").contains(&pinned),
                "EXTRACTOR_VERSION lacks {pinned}"
            );
        }
    }
}
