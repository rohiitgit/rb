# Golden fixture repositories (design §12.1)

Each directory is the *content* of a small repository. Tests turn it into a real git repository in a
temp dir with a fixed author, committer and date, so its commit SHA is stable across machines.

| Fixture | M0 checks | Later milestones |
|---|---|---|
| `ts-express-prisma` | manifest, determinism, AGE parity, CODEOWNERS evidence round trip | router prefixes, Prisma `@@map`, raw SQL, CODEOWNERS |
| `polyglot-monorepo` | SQL: Java `JdbcTemplate` with `?`, a Node `pg` insert inside a route callback (file-level attribution), Go `database/sql` with a qualified table and a dynamic-table sink (`unparsed_sql`), and migrations including `audit.users`, so that `users` is ambiguous; boundaries for every ecosystem, `rb.toml` exclude/override, CODEOWNERS edge cases (ignored duplicate file, invalid line, ownership clearing, inline comment); golden service list, cards, search and dump | web → gateway → auth → notification request path; Feign/axios stitching; Flyway |
| `java-spring-jpa` | manifest, determinism, AGE parity | `@RequestMapping` composition, interface dispatch, Spring Data, JPA `@Table`, Flyway |

Files are kept intentionally tiny; add an `expected.yaml` per fixture when M1+ extractors land.
