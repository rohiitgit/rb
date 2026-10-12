package com.acme.shop;

import dagger.Binds;
import dagger.Module;
import dagger.Provides;

@Module
public abstract class ShopModule {
    @Binds
    abstract Service bindService(ServiceImpl impl);

    @Provides
    static Db provideDb() {
        return new Db();
    }
}
