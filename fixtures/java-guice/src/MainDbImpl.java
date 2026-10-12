package com.acme.shop;

public class MainDbImpl extends MainDb {
    @Override
    public String load() {
        return "live-rows";
    }
}
