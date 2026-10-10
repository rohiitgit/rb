package com.acme.auth;

import org.springframework.context.annotation.Bean;
import org.springframework.context.annotation.Configuration;
import org.springframework.context.annotation.Import;

/** Imports the out-of-roots mail configuration (contract M6c S1). */
@Configuration
@Import(com.other.SmsConfig.class)
class AppImports {}
