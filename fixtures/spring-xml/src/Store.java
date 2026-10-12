package com.acme.shop;

public class Store {
    private final Db db;
    private Helper helper;

    public Store(Db db) {
        this.db = db;
    }

    public void setHelper(Helper helper) {
        this.helper = helper;
    }

    public String stock() {
        return db.load() + helper.help();
    }
}
