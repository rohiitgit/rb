package com.acme.shop;

import dagger.Lazy;
import jakarta.inject.Inject;

public class Shop {
    private final Service service;
    private final Lazy<Db> db;

    @Inject
    public Shop(Service service, Lazy<Db> db) {
        this.service = service;
        this.db = db;
    }

    public String run() {
        return service.serve();
    }
}
