package com.acme.auth;

import org.springframework.stereotype.Service;

@Service
public class DefaultAuthService implements AuthService {
    private final UserRepository users;
    private final PasswordHasher hasher;
    private final AuditSink audit;
    private final Notifier smsNotifier;

    public DefaultAuthService(
            UserRepository users, PasswordHasher hasher, AuditSink audit, Notifier smsNotifier) {
        this.users = users;
        this.hasher = hasher;
        this.audit = audit;
        this.smsNotifier = smsNotifier;
    }

    @Override
    public String authenticate(String email, String password) {
        audit.record(email);
        return users.findByEmail(email).map(User::getEmail).orElseThrow();
    }
}
