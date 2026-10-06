package com.acme.auth;

import org.springframework.context.annotation.Profile;
import org.springframework.stereotype.Component;

/** Issues the session token returned by a successful login. */
interface TokenIssuer {
    String issue(String user);
}

@Component
@Profile("prod")
class JwtIssuer implements TokenIssuer {
    @Override
    public String issue(String user) {
        return "jwt." + user;
    }
}

@Component
@Profile("dev")
class DevIssuer implements TokenIssuer {
    @Override
    public String issue(String user) {
        return "dev-token-" + user;
    }
}
