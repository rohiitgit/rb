package com.acme.shop;

import jakarta.inject.Inject;

public class ServiceImpl implements Service {
    @Inject
    public ServiceImpl() {
    }

    public String serve() {
        return "served";
    }
}
