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
}
