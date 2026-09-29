# Golden fixture repositories (design §12.1)

Each directory is the *content* of a small repository. Tests turn it into a real git repository in a
temp dir with a fixed author, committer and date, so its commit SHA is stable across machines.

| Fixture | M0 checks | Later milestones |
|---|---|---|
| `ts-express-prisma` | manifest, determinism, AGE parity | router prefixes, Prisma `@@map`, raw SQL, CODEOWNERS |
| `java-spring-jpa` | manifest, determinism, AGE parity | `@RequestMapping` composition, interface dispatch, Spring Data, JPA `@Table`, Flyway |

Files are kept intentionally tiny; add an `expected.yaml` per fixture when M1+ extractors land.
