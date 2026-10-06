package com.acme.auth;

import org.springframework.web.bind.annotation.*;

@RestController
@RequestMapping("/api/v1")
public class AuthController {
    private final MfaClient mfa;

    public AuthController(MfaClient mfa) {
        this.mfa = mfa;
    }

    @PostMapping("/login")
    public String login(@RequestBody String email) {
        mfa.sendCode(email);
        return "token";
    }

    @GetMapping("/login")
    public String loginPage() {
        return "login";
    }

    // @PostMapping("/logout")
    public void logout(String token) {
        // Tokens are stateless: there is no session to revoke yet.
    }
}
