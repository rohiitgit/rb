package com.acme.auth;

import org.springframework.web.bind.annotation.*;

@RestController
@RequestMapping("/api/v1")
public class AuthController {
    private final AuthService authService;

    public AuthController(AuthService authService) {
        this.authService = authService;
    }

    @PostMapping("/login")
    public String login(@RequestBody LoginRequest request) {
        return authService.authenticate(request.email(), request.password());
    }

    public record LoginRequest(String email, String password) {}
}
