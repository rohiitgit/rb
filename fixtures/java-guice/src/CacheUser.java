package com.acme.shop;

import com.google.inject.Inject;

public class CacheUser {
    private final Cache cache;

    @Inject
    public CacheUser(Cache cache) {
        this.cache = cache;
    }

    public String fetch(String key) {
        return cache.get(key);
    }
}
