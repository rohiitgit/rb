package com.acme.auth;

import java.util.List;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.beans.factory.annotation.Qualifier;
import org.springframework.web.bind.annotation.*;

@RestController
@RequestMapping("/api/v1")
public class AuthController {
    private final AuthService authService;
    private final List<Notifier> notifiers;
    private TokenIssuer issuer;

    // Accounts migrated from the old system keep plain digests until they sign in again.
    @Autowired
    @Qualifier("plainHasher")
    private PasswordHasher legacyHasher;

    public AuthController(AuthService authService, List<Notifier> notifiers) {
        this.authService = authService;
        this.notifiers = notifiers;
    }

    @Autowired
    void setIssuer(TokenIssuer i) {
        this.issuer = i;
    }

    @PostMapping("/login")
    public String login(@RequestBody LoginRequest request) {
        String user = authService.authenticate(request.email(), request.password());
        for (Notifier n : notifiers) {
            n.send(user);
        }
        return issuer.issue(user);
    }

    public record LoginRequest(String email, String password) {}
}
