package com.acme.shop;

import com.google.inject.Inject;
import com.google.inject.Singleton;
import com.google.inject.name.Named;

@Singleton
public class Store {
    private final MainDb db;

    @Inject
    public Store(@Named("main") MainDb db) {
        this.db = db;
    }

    public String stock() {
        return db.load();
    }
}
