package com.other;

import com.acme.auth.Notifier;
import org.springframework.context.annotation.Bean;
import org.springframework.context.annotation.Configuration;

/**
 * Outside the component-scan roots: its `@Bean` methods join points only as
 * registrants beside the scanned beans (contract M6c S1).
 */
@Configuration
public class SmsConfig {
    @Bean
    public Notifier sms() {
        return null;
    }
}
