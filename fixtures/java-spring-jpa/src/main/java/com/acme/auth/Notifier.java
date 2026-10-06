package com.acme.auth;

import org.springframework.stereotype.Component;

/** Tells a user that someone signed in to their account. */
public interface Notifier {
    void send(String to);
}

@Component
class EmailNotifier implements Notifier {
    @Override
    public void send(String to) {
        System.out.println("mail to " + to);
    }
}

@Component
class SmsNotifier implements Notifier {
    @Override
    public void send(String to) {
        System.out.println("sms to " + to);
    }
}
