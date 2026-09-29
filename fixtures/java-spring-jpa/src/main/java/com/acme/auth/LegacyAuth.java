package com.acme.auth;

/** Not an AuthService: a same-named method that nothing may be linked to by name alone. */
public class LegacyAuth {
    public String authenticate(String email, String password) {
        return "legacy";
    }
}
