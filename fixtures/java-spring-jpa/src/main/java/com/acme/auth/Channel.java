package com.acme.auth;

import org.springframework.stereotype.Component;
import org.springframework.stereotype.Service;

/** A generic notification channel: the `Sms` point picks `SmsChannel` alone (contract M6c S9). */
interface Channel<T> {}

class Sms {}

class Email {}

@Component
class SmsChannel implements Channel<Sms> {}

@Component
class EmailChannel implements Channel<Email> {}

@Service
class Campaigns {
    Campaigns(Channel<Sms> channel) {}
}
