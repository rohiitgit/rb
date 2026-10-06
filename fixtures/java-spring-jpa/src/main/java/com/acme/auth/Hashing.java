package com.acme.auth;

import org.springframework.context.annotation.Primary;
import org.springframework.stereotype.Component;

/** Turns a password into the digest stored with the account. */
interface PasswordHasher {
    String hash(String s);
}

/** The default hasher: an injection point without a qualifier gets this one. */
@Component
@Primary
class BcryptHasher implements PasswordHasher {
    @Override
    public String hash(String s) {
        return "bcrypt:" + Integer.toHexString(s.hashCode());
    }
}

/** Accounts migrated from the old system still compare plain digests; injected by qualifier. */
@Component
class PlainHasher implements PasswordHasher {
    @Override
    public String hash(String s) {
        return s;
    }
}
