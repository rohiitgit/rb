package com.acme.shop;

import com.google.inject.AbstractModule;
import com.google.inject.Provides;
import com.google.inject.Singleton;
import com.google.inject.name.Names;

public class ShopModule extends AbstractModule {
    @Override
    protected void configure() {
        bind(Service.class).to(ServiceImpl.class);
        bind(MainDb.class).annotatedWith(Names.named("main")).to(MainDbImpl.class);
        // Contract M6k S7: `toProvider` records the implementation like `to`.
        bind(Cache.class).toProvider(CacheProvider.class);
    }

    @Provides
    @Singleton
    CacheDb provideCache() {
        return new CacheDb();
    }
}
