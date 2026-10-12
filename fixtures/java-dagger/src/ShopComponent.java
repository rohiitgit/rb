package com.acme.shop;

import dagger.Component;
import jakarta.inject.Singleton;

@Singleton
@Component(modules = ShopModule.class)
public interface ShopComponent {
    Shop shop();
}
