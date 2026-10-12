package com.acme.shop;

import com.google.inject.Inject;
import com.google.inject.assistedinject.Assisted;

// Contract M6k S4: `@Assisted` parameters are factory inputs, never injection
// points — only `db` wires (to the `@Provides` method); `assisted` stays silent.
public class AssistedShop {
    private final CacheDb db;

    @Inject
    public AssistedShop(CacheDb db, @Assisted Service assisted) {
        this.db = db;
    }
}
