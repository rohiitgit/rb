package com.acme.auth;

import jakarta.validation.Constraint;
import jakarta.validation.ConstraintValidator;
import jakarta.validation.ConstraintValidatorContext;
import jakarta.validation.Payload;
import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;
import org.springframework.beans.factory.annotation.Autowired;

@Retention(RetentionPolicy.RUNTIME)
@Constraint(validatedBy = EmailValidator.class)
@interface ValidEmail {
    String message() default "invalid email";

    Class<?>[] groups() default {};

    Class<? extends Payload>[] payload() default {};
}

/**
 * Built by Spring's validator factory, never through component scan: its
 * injected members resolve from the validator class (contract M6c S6).
 */
class EmailValidator implements ConstraintValidator<ValidEmail, String> {
    @Autowired
    BcryptHasher hasher;

    @Override
    public boolean isValid(String value, ConstraintValidatorContext context) {
        return value != null && hasher.hash(value).length() > 0;
    }
}
