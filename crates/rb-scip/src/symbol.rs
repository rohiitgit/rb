//! Reading SCIP symbol strings (design §5.1) through the `scip` crate's parser.

/// The name of a global symbol's last descriptor: `login` for
/// `… com/acme/auth/AuthController#login().`, `users.ts` for a scip-typescript file namespace.
/// `None` for local symbols and strings the grammar rejects.
pub fn last_descriptor_name(symbol: &str) -> Option<String> {
    if scip::symbol::is_local_symbol(symbol) {
        return None;
    }
    let parsed = scip::symbol::parse_symbol(symbol).ok()?;
    parsed.descriptors.last().map(|d| d.name.clone())
}

/// Whether an occurrence of the global `symbol` may stand at a token whose text is `text`
/// (contract M3b amendment 59): its last descriptor's name is the text, or one of the forms
/// the indexers give a name that differs from the token (measured on the fixtures and on
/// spring-petclinic, class-validator, jsoup and spring-retry):
/// - a constructor, `` Foo#`<init>`(). `` (scip-java: a constructor declaration, `new Foo()`) or
///   `` Foo#`<constructor>`(). `` (scip-typescript: `new Foo()`), stands at its type's name;
/// - a scip-typescript object-literal property is named with a numeric suffix (`validate0:`
///   for `validate`);
/// - a scip-typescript accessor is named `<get>x` or `<set>x` for `x`.
///
/// `false` for local symbols and strings the grammar rejects. An import alias (a default
/// import's local name) is the caller's to accept: it needs the file's imports.
pub fn may_name(symbol: &str, text: &str) -> bool {
    NameForms::of(symbol).is_some_and(|forms| forms.names(text))
}

/// What [`may_name`] reads from a global symbol, so that a caller asking about one symbol at
/// many tokens parses it once (the `scip` crate's parser formats its own state once per
/// descriptor, so a parse costs far more than the comparisons; contract M9c P7b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameForms {
    /// The last descriptor's name.
    last: String,
    /// The name of the descriptor before it, if any.
    owner: Option<String>,
    /// Whether the last descriptor is a meta descriptor (`validate0:`).
    meta: bool,
}

impl NameForms {
    /// `None` for local symbols, strings the grammar rejects and symbols without descriptors,
    /// none of which may name a token.
    pub fn of(symbol: &str) -> Option<NameForms> {
        if scip::symbol::is_local_symbol(symbol) {
            return None;
        }
        let parsed = scip::symbol::parse_symbol(symbol).ok()?;
        let mut descriptors = parsed.descriptors.iter().rev();
        let last = descriptors.next()?;
        Some(NameForms {
            last: last.name.clone(),
            owner: descriptors.next().map(|d| d.name.clone()),
            meta: last.suffix.enum_value() == Ok(scip::types::descriptor::Suffix::Meta),
        })
    }

    /// Whether the symbol may stand at a token whose text is `text`, in the forms [`may_name`]
    /// lists.
    pub fn names(&self, text: &str) -> bool {
        let name = self.last.as_str();
        if name == text {
            return true;
        }
        if matches!(name, "<init>" | "<constructor>") {
            return self.owner.as_deref() == Some(text);
        }
        if let Some(accessed) = name
            .strip_prefix("<get>")
            .or_else(|| name.strip_prefix("<set>"))
        {
            return accessed == text;
        }
        self.meta
            && name
                .strip_prefix(text)
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    }
}

/// The symbol of a global symbol's last type: the symbol cut after its last type descriptor
/// (`… p/Dog#` for `… p/Dog#sound().`, itself for `… p/Dog#`), byte for byte a prefix of the
/// original, so it can be looked up among the index's definitions. `None` for local symbols,
/// strings the grammar rejects, symbols without a type descriptor, and a symbol the `scip`
/// crate would format differently from how the indexer wrote it (then no prefix is exact).
pub fn type_symbol_of(symbol: &str) -> Option<String> {
    if scip::symbol::is_local_symbol(symbol) {
        return None;
    }
    let mut parsed = scip::symbol::parse_symbol(symbol).ok()?;
    let last_type = parsed
        .descriptors
        .iter()
        .rposition(|d| d.suffix.enum_value() == Ok(scip::types::descriptor::Suffix::Type))?;
    parsed.descriptors.truncate(last_type + 1);
    let cut = scip::symbol::format_symbol(parsed);
    symbol.starts_with(&cut).then_some(cut)
}

/// The package a global symbol belongs to (design §5.5): the grammar's `<manager> <package-name>
/// <version>` fields.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Package {
    pub manager: String,
    /// Without scip-java's `maven/` prefix (`maven/org.springframework/spring-web` is the
    /// artifact `org.springframework/spring-web` under the manager `maven`).
    pub name: String,
    /// Empty when the indexer reported none.
    pub version: String,
}

impl Package {
    /// The `Package` node key: `<manager>/<name>@<version>`, with `?` for an unknown version.
    pub fn key(&self) -> String {
        let version = if self.version.is_empty() {
            "?"
        } else {
            &self.version
        };
        format!("{}/{}@{version}", self.manager, self.name)
    }
}

/// Whether `symbol` is Lombok's `@lombok.Generated` annotation type (contract M6a D1), the
/// marker Lombok puts on every member it generates: package `maven/org.projectlombok/lombok` of
/// any version, descriptors exactly the namespace `lombok/` and the type `Generated#`, read from
/// the parsed symbol, never from its text.
pub fn is_lombok_generated_marker(symbol: &str) -> bool {
    use scip::types::descriptor::Suffix;
    if scip::symbol::is_local_symbol(symbol) {
        return false;
    }
    let Ok(parsed) = scip::symbol::parse_symbol(symbol) else {
        return false;
    };
    let in_lombok = parsed
        .package
        .as_ref()
        .is_some_and(|p| p.manager == "maven" && p.name == "maven/org.projectlombok/lombok");
    let descriptors: Vec<(&str, Result<Suffix, i32>)> = parsed
        .descriptors
        .iter()
        .map(|d| (d.name.as_str(), d.suffix.enum_value()))
        .collect();
    in_lombok
        && descriptors
            == [
                ("lombok", Ok(Suffix::Namespace)),
                ("Generated", Ok(Suffix::Type)),
            ]
}

/// [`is_lombok_generated_marker`] for every occurrence of one index (contract M9d P9b): the
/// answer depends on the symbol string alone, and one parse costs far more than a lookup, so
/// each distinct symbol is parsed once and its answer kept for the index's other occurrences.
#[derive(Debug, Default)]
pub struct LombokMarkers<'a> {
    seen: std::collections::HashMap<&'a str, bool>,
    parses: usize,
}

impl<'a> LombokMarkers<'a> {
    /// Whether `symbol` is the Lombok marker, as [`is_lombok_generated_marker`] answers.
    pub fn is_marker(&mut self, symbol: &'a str) -> bool {
        let parses = &mut self.parses;
        *self.seen.entry(symbol).or_insert_with(|| {
            *parses += 1;
            is_lombok_generated_marker(symbol)
        })
    }

    /// How many symbols were parsed so far.
    pub fn parses(&self) -> usize {
        self.parses
    }
}

/// The package of a global symbol; `None` for local symbols, strings the grammar rejects and
/// symbols with no package (scip-java's namespace-only `scip-java maven . . org/springframework/`).
/// scip-go reports module-resolved symbols under the manager `gomod`, which normalises to the
/// manifest's `go` (contract M6d S5: one shared `go/<module>@<version>` key); every other
/// manager passes through untouched.
pub fn package(symbol: &str) -> Option<Package> {
    if scip::symbol::is_local_symbol(symbol) {
        return None;
    }
    let parsed = scip::symbol::parse_symbol(symbol).ok()?;
    let p = parsed.package.as_ref()?;
    if p.manager.is_empty() || p.name.is_empty() {
        return None;
    }
    let manager = if p.manager == "gomod" {
        "go"
    } else {
        &p.manager
    };
    // Strip with the normalized manager: a `gomod/`-prefixed wire name is
    // not a measured form, so it keys apart rather than merging across keys
    // (evaluator pass 1, E2-2 — declined the wire-manager strip: D3
    // fail-closed on unmeasured forms).
    let name = p
        .name
        .strip_prefix(&format!("{manager}/"))
        .unwrap_or(&p.name);
    Some(Package {
        manager: manager.to_owned(),
        name: name.to_owned(),
        version: p.version.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_descriptor_name_is_read_for_both_indexers_and_not_for_locals() {
        assert_eq!(
            last_descriptor_name(
                "scip-java maven maven/com.acme/auth-service 1.4.0 com/acme/auth/AuthController#login()."
            )
            .as_deref(),
            Some("login")
        );
        assert_eq!(
            last_descriptor_name("scip-java maven . . p/Dog#sound(+1).").as_deref(),
            Some("sound")
        );
        assert_eq!(
            last_descriptor_name(
                "scip-typescript npm auth-api 1.0.0 src/db/`users.ts`/findUserByEmail()."
            )
            .as_deref(),
            Some("findUserByEmail")
        );
        assert_eq!(
            last_descriptor_name("scip-typescript npm auth-api 1.0.0 src/db/`users.ts`/")
                .as_deref(),
            Some("users.ts")
        );
        assert_eq!(last_descriptor_name("local 3"), None);
        assert_eq!(last_descriptor_name("not a symbol"), None);
        assert_eq!(last_descriptor_name(""), None);
    }

    /// Contract M3b amendment 59: the forms in which an indexer's symbol names a token whose
    /// text differs from its last descriptor, and nothing else.
    #[test]
    fn a_symbol_may_name_its_token_in_the_measured_forms_only() {
        let java = "scip-java maven maven/org.jsoup/jsoup 1.24.1 org/jsoup/";
        let ts = "scip-typescript npm class-validator 0.15.1 src/";
        let yes = [
            (format!("{java}Jsoup#parse()."), "parse"),
            (format!("{java}Jsoup#`<init>`()."), "Jsoup"),
            (format!("{java}Connection#Method#`<init>`()."), "Method"),
            (
                format!(
                    "{ts}metadata/`ValidationMetadata.ts`/ValidationMetadata#`<constructor>`()."
                ),
                "ValidationMetadata",
            ),
            (
                format!("{ts}decorator/array/`ArrayContains.ts`/validate0:"),
                "validate",
            ),
            (format!("{ts}decorator/`Is6.ts`/isISO63910:"), "isISO6391"),
            (
                format!(
                    "{ts}metadata/`ConstraintMetadata.ts`/ConstraintMetadata#`<get>instance`()."
                ),
                "instance",
            ),
            (format!("{ts}`a.ts`/A#`<set>value`()."), "value"),
        ];
        for (symbol, text) in &yes {
            assert!(may_name(symbol, text), "{symbol} at {text}");
        }
        let no = [
            (format!("{java}LoneCr2#m0()."), "m2"),
            (format!("{java}LoneCr2#zeta()."), "beta"),
            (format!("{java}Jsoup#`<init>`()."), "Other"),
            (format!("{java}Jsoup#`<init>`()."), "init"),
            (format!("{ts}`a.ts`/validate0."), "validate"),
            (format!("{ts}`a.ts`/validate:"), "valid"),
            (format!("{ts}`a.ts`/validatex:"), "validate"),
            (format!("{ts}`a.ts`/A#`<get>value`()."), "<get>"),
            ("local 3".to_owned(), "m"),
            ("not a symbol".to_owned(), "not"),
        ];
        for (symbol, text) in &no {
            assert!(!may_name(symbol, text), "{symbol} at {text}");
        }
    }

    /// The type symbol is the symbol cut after its last type descriptor (a member's enclosing
    /// type, or the type itself), byte for byte a prefix of the original, so it can be looked up
    /// among the index's definitions; a top-level function, a namespace and a local have none.
    #[test]
    fn the_type_symbol_is_the_symbol_cut_after_its_last_type_descriptor() {
        let java = "scip-java maven maven/com.acme/auth-service 1.4.0 com/acme/auth/";
        assert_eq!(
            type_symbol_of(&format!("{java}DefaultAuthService#authenticate().")).as_deref(),
            Some(format!("{java}DefaultAuthService#").as_str())
        );
        assert_eq!(
            type_symbol_of(&format!("{java}AuthService#")).as_deref(),
            Some(format!("{java}AuthService#").as_str())
        );
        assert_eq!(
            type_symbol_of(&format!("{java}Outer#Inner#run(+1).")).as_deref(),
            Some(format!("{java}Outer#Inner#").as_str())
        );
        assert_eq!(
            type_symbol_of("scip-java maven . . p/Dog#sound().").as_deref(),
            Some("scip-java maven . . p/Dog#")
        );
        assert_eq!(
            type_symbol_of("scip-typescript npm p 1.0.0 src/`cache.ts`/Cache#get().").as_deref(),
            Some("scip-typescript npm p 1.0.0 src/`cache.ts`/Cache#")
        );
        assert_eq!(
            type_symbol_of("scip-typescript npm p 1.0.0 src/`a.ts`/f()."),
            None
        );
        assert_eq!(type_symbol_of(java), None);
        assert_eq!(type_symbol_of("local 3"), None);
        assert_eq!(type_symbol_of("not a symbol"), None);
    }

    #[test]
    fn packages_come_from_the_symbol_and_key_as_manager_name_version() {
        let pkg = |manager: &str, name: &str, version: &str| Package {
            manager: manager.into(),
            name: name.into(),
            version: version.into(),
        };
        // scip-typescript names the npm package as is.
        let prisma = package("scip-typescript npm @prisma/client 5.22.0 `default.d.ts`/").unwrap();
        assert_eq!(prisma, pkg("npm", "@prisma/client", "5.22.0"));
        assert_eq!(prisma.key(), "npm/@prisma/client@5.22.0");
        // scip-java prefixes the artifact with `maven/`, which the key drops; the JDK is
        // `jdk <major>`.
        let spring = package(
            "scip-java maven maven/org.springframework/spring-web 6.1.13 org/springframework/web/bind/annotation/GetMapping#",
        )
        .unwrap();
        assert_eq!(
            spring,
            pkg("maven", "org.springframework/spring-web", "6.1.13")
        );
        assert_eq!(spring.key(), "maven/org.springframework/spring-web@6.1.13");
        assert_eq!(
            package("scip-java maven jdk 21 java/lang/String#")
                .unwrap()
                .key(),
            "maven/jdk@21"
        );
        // A version the indexer did not report.
        assert_eq!(
            package("scip-java maven maven/org.ext/lib . e/Ext#")
                .unwrap()
                .key(),
            "maven/org.ext/lib@?"
        );
        // A prefix that is not the manager stays.
        assert_eq!(
            package("scip-java maven npm/x 1 a/B#").unwrap().name,
            "npm/x"
        );
        // No package: namespace-only symbols, locals, and strings the grammar rejects.
        assert_eq!(package("scip-java maven . . org/springframework/"), None);
        assert_eq!(package("scip-java . x 1 a/B#"), None);
        assert_eq!(package("local 3"), None);
        assert_eq!(package("not a symbol"), None);
        assert_eq!(package(""), None);
    }

    /// Contract M6d S5: scip-go reports module-resolved symbols under the
    /// manager `gomod`, which keys as the manifest's `go` (one shared
    /// `go/<module>@<version>` key); names and versions pass through
    /// byte-identical, and a version the index did not report keys apart
    /// (`?`), so a mismatch keeps both rows (D3).
    #[test]
    fn go_packages_key_as_the_manifest_does() {
        let pkg = |manager: &str, name: &str, version: &str| Package {
            manager: manager.into(),
            name: name.into(),
            version: version.into(),
        };
        // Measured with rb-scip-go:0.2.7 on a gin-bearing module: a dependency
        // carries its module path and resolved version.
        let gin = package("scip-go gomod github.com/gin-gonic/gin v1.10.0 gin/Context#").unwrap();
        assert_eq!(gin, pkg("go", "github.com/gin-gonic/gin", "v1.10.0"));
        assert_eq!(gin.key(), "go/github.com/gin-gonic/gin@v1.10.0");
        // The standard library is a pseudo-module at the toolchain version.
        let std = package("scip-go gomod github.com/golang/go/src go1.23 net/http#Get().").unwrap();
        assert_eq!(std.key(), "go/github.com/golang/go/src@go1.23");
        // The main module reports no version (`.` parses as absent, as on
        // scip-java's `maven/org.ext/lib .` above).
        let own = package("scip-go gomod github.com/acme/billing . billing/Handler#").unwrap();
        assert_eq!(own.key(), "go/github.com/acme/billing@?");
        // A `gomod/`-prefixed wire name is not ours to shorten: it keys
        // apart (evaluator pass 1, E2-2), never merging across keys.
        assert_eq!(
            package("scip-go gomod gomod/github.com/gin-gonic/gin v1.10.0 gin/Context#")
                .unwrap()
                .key(),
            "go/gomod/github.com/gin-gonic/gin@v1.10.0"
        );
        // npm and maven managers pass through untouched.
        assert_eq!(
            package("scip-typescript npm gin 1.0.0 src/`a.ts`/")
                .unwrap()
                .key(),
            "npm/gin@1.0.0"
        );
    }

    /// Contract M6d S5/D3: the same module at different versions keys apart,
    /// so each row survives (never merged across keys).
    #[test]
    fn go_packages_at_different_versions_key_apart() {
        let at = |version: &str| {
            package(&format!(
                "scip-go gomod github.com/gin-gonic/gin {version} gin/Context#"
            ))
            .unwrap()
            .key()
        };
        assert_eq!(at("v1.10.0"), "go/github.com/gin-gonic/gin@v1.10.0");
        assert_eq!(at("v1.9.1"), "go/github.com/gin-gonic/gin@v1.9.1");
        assert_ne!(at("v1.10.0"), at("v1.9.1"));
        // An unreported version (`.` parses as absent) keys apart from every
        // pinned one.
        assert_eq!(at("."), "go/github.com/gin-gonic/gin@?");
        assert_ne!(at("."), at("v1.10.0"));
    }
    /// told apart by the parsed package (`maven/org.projectlombok/lombok`, any version) and the
    /// whole descriptor path (`lombok/` then `Generated#`), never by text.
    /// Contract M6a D1, S1 (k): the marker is Lombok's own `lombok.Generated` annotation type,
    /// told apart by the parsed package (`maven/org.projectlombok/lombok`, any version) and the
    /// whole descriptor path (`lombok/` then `Generated#`), never by text.
    #[test]
    fn the_lombok_marker_is_identified_by_package_and_descriptors() {
        let yes = [
            "scip-java maven maven/org.projectlombok/lombok 1.18.34 lombok/Generated#",
            "scip-java maven maven/org.projectlombok/lombok 1.18.30 lombok/Generated#",
        ];
        for symbol in yes {
            assert!(is_lombok_generated_marker(symbol), "{symbol}");
        }
        let no = [
            // Another package that declares the same type name.
            "scip-java maven maven/com.example/fake 1.0 lombok/Generated#",
            // No package at all.
            "scip-java maven . . lombok/Generated#",
            // Other types and members of Lombok, and the namespace alone.
            "scip-java maven maven/org.projectlombok/lombok 1.18.34 lombok/Getter#",
            "scip-java maven maven/org.projectlombok/lombok 1.18.34 lombok/experimental/Generated#",
            "scip-java maven maven/org.projectlombok/lombok 1.18.34 lombok/Generated#value().",
            "scip-java maven maven/org.projectlombok/lombok 1.18.34 lombok/",
            // javax's and the JDK's `Generated`.
            "scip-java maven jdk 21 javax/annotation/processing/Generated#",
            "scip-java maven maven/javax.annotation/javax.annotation-api 1.3.2 javax/annotation/Generated#",
            "local 4",
            "not a symbol",
        ];
        for symbol in no {
            assert!(!is_lombok_generated_marker(symbol), "{symbol}");
        }
    }

    /// Contract M9d P9b: the marker check parses each distinct symbol once, however often it
    /// occurs, and answers every occurrence as `is_lombok_generated_marker` does.
    #[test]
    fn lombok_markers_parse_each_distinct_symbol_once() {
        let marker = "scip-java maven maven/org.projectlombok/lombok 1.18.34 lombok/Generated#";
        let getter = "scip-java maven maven/org.projectlombok/lombok 1.18.34 lombok/Getter#";
        let own = "scip-java maven maven/com.acme/demo 1.0 p/Big#f().";
        let occurrences = [
            marker,
            own,
            getter,
            marker,
            own,
            "local 4",
            marker,
            own,
            "not a symbol",
            "local 4",
        ];
        let mut memo = LombokMarkers::default();
        for symbol in occurrences {
            assert_eq!(
                memo.is_marker(symbol),
                is_lombok_generated_marker(symbol),
                "{symbol}"
            );
        }
        let distinct: std::collections::BTreeSet<&str> = occurrences.into_iter().collect();
        assert_eq!(memo.parses(), distinct.len());
        // Asked again, nothing more is parsed.
        assert!(memo.is_marker(marker) && !memo.is_marker(own));
        assert_eq!(memo.parses(), distinct.len());
    }
}
