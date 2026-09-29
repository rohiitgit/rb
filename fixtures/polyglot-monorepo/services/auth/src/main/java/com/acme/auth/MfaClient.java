package com.acme.auth;

import org.springframework.cloud.openfeign.FeignClient;
import org.springframework.web.bind.annotation.PostMapping;

@FeignClient(name = "notification", url = "http://notification:3000")
public interface MfaClient {
    @PostMapping("/internal/mfa/email")
    void sendCode(String email);
}
