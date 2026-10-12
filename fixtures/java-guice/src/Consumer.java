package com.acme.shop;

import com.google.inject.Inject;

public class Consumer {
    private final Service service;

    @Inject
    public Consumer(Service service) {
        this.service = service;
    }

    public String run() {
        return service.serve();
    }
}
