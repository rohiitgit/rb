package com.acme.auth;

import com.other.SmsConfig;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.context.annotation.Import;

/**
 * A test source naming the config by class literal: it caps nothing in the
 * application context (contract M6c S2).
 */
@Import(SmsConfig.class)
class AlertsTest {
    @Autowired
    Notifier notifier;
}
