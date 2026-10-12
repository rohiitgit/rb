package com.acme.auth;

import org.springframework.context.annotation.Bean;
import org.springframework.context.annotation.Configuration;

@Configuration
public class AuditConfig {
    @Bean
    AuditSink auditSink() {
        return new LogAuditSink();
    }

    @Bean
    JwtFilter jwtFilter() {
        return new JwtFilter();
    }
}

/** Receives one line per security-relevant event. */
interface AuditSink {
    void record(String event);
}

class LogAuditSink implements AuditSink {
    @Override
    public void record(String event) {
        System.out.println("audit: " + event);
    }
}

/** Meant to publish audit events to a topic once the broker is provisioned; not wired yet. */
class KafkaAuditSink implements AuditSink {
    @Override
    public void record(String event) {
        throw new UnsupportedOperationException("no broker configured");
    }
}
