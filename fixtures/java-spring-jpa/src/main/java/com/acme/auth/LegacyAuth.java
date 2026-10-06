package com.acme.auth;

import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestMapping;

/**
 * Not an AuthService: a same-named method that nothing may be linked to by name alone. Its
 * mappings are inert: without {@code @RestController} or {@code @Controller}, Spring 6 does not
 * register the class as a handler.
 */
@RequestMapping("/legacy")
public class LegacyAuth {
    @PostMapping("/login")
    public String authenticate(String email, String password) {
        return "legacy";
    }
}
