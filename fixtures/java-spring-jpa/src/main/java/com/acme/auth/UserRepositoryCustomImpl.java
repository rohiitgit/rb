package com.acme.auth;

import java.time.Instant;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/** Spring Data finds this fragment by its name and mixes it into the repository proxy. */
class UserRepositoryCustomImpl implements UserRepositoryCustom {
    private final Map<Long, Instant> lastSeen = new ConcurrentHashMap<>();

    @Override
    public void touch(long id) {
        lastSeen.put(id, Instant.now());
    }
}
