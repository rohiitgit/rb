package com.acme.auth;

public interface AuthService {
    String authenticate(String email, String password);
}
