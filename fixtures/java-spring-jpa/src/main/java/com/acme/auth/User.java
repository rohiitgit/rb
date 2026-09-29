package com.acme.auth;

import jakarta.persistence.*;

@Entity
@Table(name = "users")
public class User {
    @Id
    private Long id;
    private String email;

    public String getEmail() {
        return email;
    }
}
