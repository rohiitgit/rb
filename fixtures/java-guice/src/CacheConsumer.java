package com.acme.shop;

import com.google.inject.Inject;

public class CacheConsumer {
    private final CacheDb cache;

    @Inject
    public CacheConsumer(CacheDb cache) {
        this.cache = cache;
    }
}
