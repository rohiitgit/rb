package com.acme.auth;

import org.springframework.stereotype.Service;

@Service
public class DefaultAuthService implements AuthService {
    private final UserRepository users;

    public DefaultAuthService(UserRepository users) {
        this.users = users;
    }

    @Override
    public String authenticate(String email, String password) {
        return users.findByEmail(email).map(User::getEmail).orElseThrow();
    }
}
