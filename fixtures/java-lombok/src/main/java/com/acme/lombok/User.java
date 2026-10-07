package com.acme.lombok;

import lombok.AllArgsConstructor;
import lombok.Data;
import lombok.NoArgsConstructor;

@Data
@NoArgsConstructor
@AllArgsConstructor
public class User {
    private String name;
    private boolean active;

    public String greet() {
        return getName() + helper();
    }

    private String helper() {
        return "!";
    }
}
