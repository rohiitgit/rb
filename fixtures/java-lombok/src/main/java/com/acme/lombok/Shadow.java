package com.acme.lombok;

import lombok.Getter;

public class Shadow {
    public String run() {
        return new Named().getName() + new Explicit().getName();
    }
}

class Named {
    @Getter String name;
}

class Explicit {
    String getName() {
        return "e";
    }
}
