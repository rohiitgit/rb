package com.acme.auth;

import org.springframework.beans.factory.annotation.Autowired;

/**
 * A servlet filter the container builds through the {@code jwtFilter} factory
 * in {@link AuditConfig}, never through component scan: its injected members
 * resolve from the factory method (contract M6c S6).
 */
class JwtFilter {
    @Autowired
    BcryptHasher hasher;
}
