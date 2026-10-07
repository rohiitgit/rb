package com.acme.lombok;

import lombok.EqualsAndHashCode;
import lombok.ToString;

@EqualsAndHashCode
@ToString
public class Plain {
    private int base = 2;

    public int calc() {
        return base * 3;
    }
}
