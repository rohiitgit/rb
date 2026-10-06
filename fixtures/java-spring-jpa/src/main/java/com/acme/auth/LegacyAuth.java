package com.acme.auth;

import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestMapping;

/**
 * Not an AuthService: a same-named method that nothing may be linked to by name alone. Its
 * mappings are inert: without {@code @RestController} or {@code @Controller}, Spring 6 does not
 * register the class as a handler.
 */
// @Component
@RequestMapping("/legacy")
public class LegacyAuth {
    @Autowired
    private AuthService delegate;

    @PostMapping("/login")
    public String authenticate(String email, String password) {
        return "legacy:" + LegacyDigest.legacyHash(password);
    }
}

/** The old system's password digest, kept until every migrated account has signed in again. */
final class LegacyDigest {
    private LegacyDigest() {}

    static String legacyHash(String password) {
        return Integer.toHexString(password.hashCode());
    }
}
