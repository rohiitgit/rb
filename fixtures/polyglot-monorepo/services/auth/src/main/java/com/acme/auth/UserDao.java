package com.acme.auth;

import org.springframework.jdbc.core.JdbcTemplate;

public class UserDao {
    private final JdbcTemplate jdbc;

    public UserDao(JdbcTemplate jdbc) {
        this.jdbc = jdbc;
    }

    public String emailOf(long id) {
        return jdbc.queryForObject("SELECT email FROM users WHERE id = ?", String.class, id);
    }

    public void touch(long id) {
        jdbc.update("UPDATE users SET last_seen = now() WHERE id = ?", id);
        jdbc.update("INSERT INTO audit.users (user_id, event) VALUES (?, 'touch')", id);
    }
}
