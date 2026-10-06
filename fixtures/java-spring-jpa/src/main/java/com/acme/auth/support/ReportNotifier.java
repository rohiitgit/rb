package com.acme.auth.support;

import com.acme.auth.Notifier;

@com.acme.auth.support.Service
public class ReportNotifier implements Notifier {
    @Override
    public void send(String to) {
        System.out.println("report for " + to);
    }
}
