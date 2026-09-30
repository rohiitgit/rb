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
}
