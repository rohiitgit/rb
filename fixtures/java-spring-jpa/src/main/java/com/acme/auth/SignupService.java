package com.acme.auth;

import org.springframework.stereotype.Service;

/** Registers accounts through the inherited Spring Data operations (contract M6c S7). */
@Service
class SignupService {
    private final UserRepository users;

    SignupService(UserRepository users) {
        this.users = users;
    }

    void register(User user) {
        users.save(user);
    }

    void lookup(long id) {
        users.findById(id);
    }
}
