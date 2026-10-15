# hk_polls hand key: labelling

Repo pinned at 362fad90cab17e76453b3b9e273c594de6ee3d7f. Backend: `polling-app-server/` (Spring Boot, Spring Data JPA, MySQL).
`application.properties` sets no `server.servlet.context-path` / `server.contextPath`, and the deployment yamls and docker-compose set no equivalent env var, so the context path is empty.
Prefix abbreviation below: `P = polling-app-server/src/main/java/com/example/polls`.

## ENDPOINT (12)

AuthController: class-level `@RequestMapping("/api/auth")` (AuthController.java:33)
- `ENDPOINT POST /api/auth/signin …/controller/AuthController.java:52`: line 51 `@PostMapping("/signin")`; line 52 `public ResponseEntity<?> authenticateUser(`
- `ENDPOINT POST /api/auth/signup …/controller/AuthController.java:68`: line 67 `@PostMapping("/signup")`; line 68 `registerUser(`

PollController: class-level `@RequestMapping("/api/polls")` (PollController.java:23)
- `ENDPOINT GET /api/polls …/PollController.java:41`: line 40 `@GetMapping` (no path); line 41 `getPolls(`
- `ENDPOINT POST /api/polls …/PollController.java:49`: line 47 `@PostMapping` (no path); line 49 `createPoll(` (line 48 is `@PreAuthorize`)
- `ENDPOINT GET /api/polls/{pollId} …/PollController.java:61`: line 60 `@GetMapping("/{pollId}")`; line 61 `getPollById(`
- `ENDPOINT POST /api/polls/{pollId}/votes …/PollController.java:68`: line 66 `@PostMapping("/{pollId}/votes")`; line 68 `castVote(`

UserController: class-level `@RequestMapping("/api")` (UserController.java:20)
- `ENDPOINT GET /api/user/me …/UserController.java:39`: line 37 `@GetMapping("/user/me")`; line 39 `getCurrentUser(`
- `ENDPOINT GET /api/user/checkUsernameAvailability …/UserController.java:45`: line 44 `@GetMapping("/user/checkUsernameAvailability")`; line 45 `checkUsernameAvailability(`
- `ENDPOINT GET /api/user/checkEmailAvailability …/UserController.java:51`: line 50 `@GetMapping("/user/checkEmailAvailability")`; line 51 `checkEmailAvailability(`
- `ENDPOINT GET /api/users/{username} …/UserController.java:57`: line 56 `@GetMapping("/users/{username}")`; line 57 `getUserProfile(`
- `ENDPOINT GET /api/users/{username}/polls …/UserController.java:70`: line 69 `@GetMapping("/users/{username}/polls")`; line 70 `getPollsCreatedBy(`
- `ENDPOINT GET /api/users/{username}/votes …/UserController.java:78`: line 77 `@GetMapping("/users/{username}/votes")`; line 78 `getPollsVotedBy(`

There are no other request mappings. `WebMvcConfig.java:18` `registry.addMapping("/**")` is a CORS mapping, not a route.

## QUERIES (20)

PollRepository: `extends JpaRepository<Poll, Long>` (PollRepository.java:14). Poll is mapped to `@Table(name = "polls")`.
- `QUERIES polls read …/PollRepository.java:16`: `Optional<Poll> findById(Long pollId);` is explicitly declared in the source, which overrides the inherited CRUD method (see Uncertain)
- `QUERIES polls read …/PollRepository.java:18`: `Page<Poll> findByCreatedBy(Long userId, Pageable pageable);` is a derived find
- `QUERIES polls read …/PollRepository.java:20`: `long countByCreatedBy(Long userId);` is a derived count, so read
- `QUERIES polls read …/PollRepository.java:22`: `List<Poll> findByIdIn(List<Long> pollIds);` is a derived find
- `QUERIES polls read …/PollRepository.java:24`: `List<Poll> findByIdIn(List<Long> pollIds, Sort sort);` is an overload with its own declaration line

RoleRepository: `extends JpaRepository<Role, Long>` (RoleRepository.java:11). Role is mapped to `@Table(name = "roles")`.
- `QUERIES roles read …/RoleRepository.java:12`: `Optional<Role> findByName(RoleName roleName);`

UserRepository: `extends JpaRepository<User, Long>` (UserRepository.java:11). User is mapped to `@Table(name = "users", …)`.
- `QUERIES users read …/UserRepository.java:12`: `Optional<User> findByEmail(String email);`
- `QUERIES users read …/UserRepository.java:14`: `Optional<User> findByUsernameOrEmail(String username, String email);`
- `QUERIES users read …/UserRepository.java:16`: `List<User> findByIdIn(List<Long> userIds);`
- `QUERIES users read …/UserRepository.java:18`: `Optional<User> findByUsername(String username);`
- `QUERIES users read …/UserRepository.java:20`: `Boolean existsByUsername(String username);` is a derived exists, so read
- `QUERIES users read …/UserRepository.java:22`: `Boolean existsByEmail(String email);` is a derived exists, so read

VoteRepository: JPQL `@Query` on every method; entity `Vote` is mapped to `@Table(name = "votes", …)`.
- `QUERIES votes read …/VoteRepository.java:17`: line 16 `@Query("SELECT NEW …ChoiceVoteCount(v.choice.id, count(v.id)) FROM Vote v WHERE v.poll.id in :pollIds GROUP BY v.choice.id")`; line 17 `countByPollIdInGroupByChoiceId(`
- `QUERIES votes read …/VoteRepository.java:20`: line 19 `@Query("SELECT NEW … FROM Vote v WHERE v.poll.id = :pollId GROUP BY v.choice.id")`; line 20 `countByPollIdGroupByChoiceId(`
- `QUERIES votes read …/VoteRepository.java:23`: line 22 `@Query("SELECT v FROM Vote v where v.user.id = :userId and v.poll.id in :pollIds")`; line 23 `findByUserIdAndPollIdIn(`
- `QUERIES votes read …/VoteRepository.java:26`: line 25 `@Query("SELECT v FROM Vote v where v.user.id = :userId and v.poll.id = :pollId")`; line 26 `findByUserIdAndPollId(`
- `QUERIES votes read …/VoteRepository.java:29`: line 28 `@Query("SELECT COUNT(v.id) from Vote v where v.user.id = :userId")`; line 29 `countByUserId(`
- `QUERIES votes read …/VoteRepository.java:32`: line 31 `@Query("SELECT v.poll.id FROM Vote v WHERE v.user.id = :userId")`; line 32 `findVotedPollIdsByUserId(`

File-level SQL (DML only)
- `QUERIES roles insert polling-app-server/src/main/resources/data.sql:0`: lines 1-2 `INSERT IGNORE INTO roles(name) VALUES('ROLE_USER');` and `…('ROLE_ADMIN');`
- `QUERIES roles insert polling-app-server/src/main/resources/db/migration/V2__default_roles.sql:0`: lines 1-2 contain the same `INSERT IGNORE INTO roles(name) …` statements

Excluded:
- `db/migration/V1__schema.sql` contains only `CREATE TABLE` statements, which are DDL.
- The service layer (`PollService`) and `CustomUserDetailsService` only call repository methods. These include inherited `findAll`, `save`, `getOne` and `findById` on UserRepository, which is not declared there. Calls of this kind are not lines.
- There is no JdbcTemplate, EntityManager or native query anywhere in `src/main` (grep for `jdbc|createQuery|nativeQuery|EntityManager` found nothing).

## MAPS (5)
- `MAPS choices …/model/Choice.java:10`: lines 8-9 `@Entity` `@Table(name = "choices")`; line 10 `public class Choice {`
- `MAPS polls …/model/Poll.java:17`: lines 15-16 `@Entity` `@Table(name = "polls")`; line 17 `public class Poll extends UserDateAudit {`
- `MAPS roles …/model/Role.java:8`: lines 6-7 `@Entity` `@Table(name = "roles")`; line 8 `public class Role {`
- `MAPS users …/model/User.java:21`: lines 12-13 `@Entity` `@Table(name = "users", uniqueConstraints = …)`; line 21 `public class User extends DateAudit {`
- `MAPS votes …/model/Vote.java:13`: lines 6-7 `@Entity` `@Table(name = "votes", uniqueConstraints = …)`; line 13 `public class Vote extends DateAudit {`

Excluded:
- `model/audit/DateAudit.java:19` and `UserDateAudit.java:16` are `@MappedSuperclass public abstract class`.
- `ChoiceVoteCount` is a plain DTO with no annotation.
- `RoleName` is an enum.
- The `user_roles` join table (`@JoinTable(name = "user_roles")`, User.java:45) has no entity class.

## Uncertain
1. **PollRepository.java:16 `findById`**: the rules say inherited CRUD methods that the source *does not declare* are not lines. This one *is* redeclared in the interface source, so I included it as `polls read`. Drop it if redeclared CRUD methods are to be treated as inherited.
2. **JPQL association paths in VoteRepository** (`v.choice.id`, `v.poll.id`, `v.user.id`): each one references only the foreign-key id of a `@ManyToOne`. Hibernate resolves these to the `votes.choice_id` / `poll_id` / `user_id` columns with no join, so I listed only `votes`. A stricter reading of "joined entities" would add `choices`, `polls` and `users` lines for the affected methods:
   - choices for :17 and :20
   - polls for :17, :20, :23, :26 and :32
   - users for :23, :26, :29 and :32
3. **The two file-level role INSERT files** (`data.sql` and the Flyway-style `V2__default_roles.sql`): both contain DML against `roles`, so both are listed.
   - `pom.xml` decides which one actually runs; `spring.datasource.initialization-mode=always` runs `data.sql`.
   - They are listed regardless, because the rule counts any .sql file with DML.
4. **`countByUserId` (VoteRepository.java:29)**: its name looks like a derived query, but it carries an explicit `@Query`. It is labelled once as `votes read` either way.
