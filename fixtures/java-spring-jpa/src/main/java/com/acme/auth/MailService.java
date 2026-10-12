package com.acme.auth;

import lombok.RequiredArgsConstructor;
import org.springframework.beans.factory.annotation.Qualifier;
import org.springframework.stereotype.Service;

/**
 * Constructor-injected through Lombok: the field `@Qualifier` rides the
 * generated parameter because `lombok.config` lists it (contract M6c S8).
 */
@Service
@RequiredArgsConstructor
class MailService {
    @Qualifier("plainHasher")
    private final PasswordHasher hasher;
}
