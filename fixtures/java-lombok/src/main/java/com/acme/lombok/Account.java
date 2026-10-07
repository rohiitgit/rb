package com.acme.lombok;

import lombok.Getter;
import lombok.Setter;

@Getter
@Setter
public class Account {
    private String owner;
    @Getter private long id;

    public long twice() {
        return getId() * 2;
    }
}
