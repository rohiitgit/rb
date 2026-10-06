package com.acme.auth;

import java.util.Optional;
import org.springframework.data.jpa.repository.JpaRepository;

public interface UserRepository extends JpaRepository<User, Long>, UserRepositoryCustom {
    Optional<User> findByEmail(String email);
}

/** Queries Spring Data cannot derive from a method name; implemented by hand. */
interface UserRepositoryCustom {
    void touch(long id);
}
