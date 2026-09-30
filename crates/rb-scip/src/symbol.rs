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

/// The package of a global symbol; `None` for local symbols, strings the grammar rejects and
/// symbols with no package (scip-java's namespace-only `scip-java maven . . org/springframework/`).
pub fn package(symbol: &str) -> Option<Package> {
    if scip::symbol::is_local_symbol(symbol) {
        return None;
    }
    let parsed = scip::symbol::parse_symbol(symbol).ok()?;
    let p = parsed.package.as_ref()?;
    if p.manager.is_empty() || p.name.is_empty() {
        return None;
    }
    let name = p
        .name
        .strip_prefix(&format!("{}/", p.manager))
        .unwrap_or(&p.name);
    Some(Package {
        manager: p.manager.clone(),
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
}
