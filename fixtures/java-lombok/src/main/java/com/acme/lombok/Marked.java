package com.acme.lombok;

public class Marked {
    @lombok.Generated
    public int value() {
        return 1;
    }

    public int use() {
        return value();
    }
}
