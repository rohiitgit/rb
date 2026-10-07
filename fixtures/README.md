# Golden fixture repositories (design §12.1)

Each directory is the *content* of a small repository. Tests turn it into a real git repository in a
temp dir with a fixed author, committer and date, so its commit SHA is stable across machines.

| Fixture | M0 checks | Later milestones |
|---|---|---|
| `ts-express-prisma` | manifest, determinism, AGE parity, CODEOWNERS evidence round trip | router prefixes, Prisma `@@map`, raw SQL, CODEOWNERS |
| `polyglot-monorepo` | SQL: Java `JdbcTemplate` with `?`, a Node `pg` insert inside a route callback (file-level attribution), Go `database/sql` with a qualified table and a dynamic-table sink (`unparsed_sql`), and migrations including `audit.users`, so that `users` is ambiguous; boundaries for every ecosystem, `rb.toml` exclude/override, CODEOWNERS edge cases (ignored duplicate file, invalid line, ownership clearing, inline comment); golden service list, cards, search and dump. SCIP (M3b): `services/auth` builds with Maven and indexes exactly; `services/broken` depends on `com.acme:nope:0.0.0`, which does not exist, so its job fails on purpose and it stays heuristic while the run succeeds | web → gateway → auth → notification request path; Feign/axios stitching; Flyway |
| `java-spring-jpa` | manifest, determinism, AGE parity | `@RequestMapping` composition, interface dispatch, Spring Data, JPA `@Table`, Flyway |
| `java-lombok` | — | M6a (contract S2, S4): Lombok 1.18.34 (`provided`, fetched from Maven Central through the egress proxy) on `@Data`/`@NoArgsConstructor`/`@AllArgsConstructor` (`User`), class- and field-level `@Getter`/`@Setter` (`Account`), `@Value`/`@With`/`@Builder` (`Money`), `@EqualsAndHashCode`/`@ToString` (`Plain`), S2's shadow (`Shadow.java`: package-private `Named` with a field `@Getter` declared before `Explicit`, whose `getName()` is where scip-java defines `Named`'s generated getter), a hand-written `@lombok.Generated` method (`Marked`), and `Use` calling each. SCIP: every document accepted after D1 (`lombok-generated 234 in 6 documents`); goldens `scip-status.txt`, `scip-callers-greet.txt`, `dump.jsonl` (`--no-scip`), SCIP guards `must-exist.txt`/`must-not-exist.txt` |

Files are kept intentionally tiny; add an `expected.yaml` per fixture when M1+ extractors land.
