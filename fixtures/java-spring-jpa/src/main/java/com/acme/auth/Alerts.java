package com.acme.auth;

import org.springframework.stereotype.Service;

/** A consumer beside the `@Import`-named config's `@Bean` method (contract M6c S1). */
@Service
class Alerts {
    Alerts(Notifier notifier) {}
}
