# Hand-key format (contract M8b, T3)

A hand key states, from the source alone, every HTTP endpoint and every table access in one
repository at one pinned commit. It is written by someone who has not seen any tool's output for
that repository. One line per fact, in `bench/keys/<target>/key.txt`; `#` starts a comment.

```
ENDPOINT <METHOD> <full path> <file>:<line>
QUERIES <table> <ops> <file>:<line>
MAPS <table> <file>:<line>
```

Paths are repository-relative, as `git ls-files` prints them. Lines are 1-based. Skip test code
(`src/test/`, `*_test.go`, `test_*.py`, `*Test.java`, `tests/` directories).

## ENDPOINT

One line per HTTP method and route that the application serves.

- **METHOD:** `GET`, `POST`, `PUT`, `DELETE`, `PATCH` and so on, in upper case. A mapping that accepts every method (`@RequestMapping` with no `method`) is `ANY`.
- **Full path:** the path a client requests. That means:
  - the class-level prefix, the router group or sub-router prefix, and the framework context path (`server.servlet.context-path`, `server.contextPath`) joined in front;
  - path parameters written as they are in the source (`/users/{id}`, `/users/:id`).
- **Location:** `<file>:<line>` is the handler.
  - A named function or method: the line of its **name**, where it is declared (`public User getUser(...)`, `func (h *Handler) getUser(...)`).
  - An anonymous handler (a lambda or function literal): the line where it starts.
- **One line per mapping:** a mapping with several paths or methods gives one line each.
- **Library handlers:** a handler the repository registers but does not declare (a library's handler, such as Swagger UI) is located at the registration line, and `labelling.md` marks it as a library handler.

## QUERIES

One line per (function, table) where a function's own code states SQL against a table:

- a SQL string passed to a database call (`db.Query`, `jdbcTemplate`, `sqlx`'s `Get`/`Select`/`Exec`, …). The table is resolved through constants or `fmt.Sprintf` when the code builds the name;
- an `@Query` annotation (JPQL or native) on a repository method: the table of the entity, or the SQL's table;
- a Spring Data **derived query method declared in the source** (`findByUsername`, `existsByEmail`, `countBy…`, `deleteBy…`): the repository's entity's table.

**Not lines:**
- calls to inherited CRUD methods that the source does not declare (`save`, `findById`, `findAll`);
- callers of a query function: only the function holding the SQL or declaring the query counts.

**Fields:**
- **`<table>`:** the table name, lower case; `schema.table` only when the source writes the schema.
- **`<ops>`:** comma-separated from `read`, `insert`, `update`, `delete`, by SQL verb. Derived `find`/`exists`/`count`/`get` methods are `read`, and `delete`/`remove` ones are `delete`.
- **`<file>:<line>`:** the function's name line, or `<file>:0` when the SQL sits at file level (a `.sql` file, a mapper XML).
- **DML only:** DDL (`CREATE`/`ALTER`/`DROP TABLE`, as in migrations) is not a query. Only DML (`SELECT`/`INSERT`/`UPDATE`/`DELETE`) counts. A function touching several tables (a join, several statements) gets one line per table, with that table's ops; a joined table read by a `SELECT` is `read`.

## MAPS

One line per entity class mapped to a table:
- **The class:** a JPA `@Entity`, or a class whose ORM mapping names a table.
- **`<table>`:** the name from `@Table(name = …)`, else the framework default. Spring Boot's default is the class name in `snake_case`, lower case.
- **`<file>:<line>`:** the line of the class's name.

## Adjudication (T4)

`key.txt` is the labeller's output and is never edited. `adjudication.md` is append-only: each
entry is a line `- ADD `<key line>`` or `- REMOVE `<key line>``, followed by its file:line
evidence. The effective key is `key.txt` with every entry applied in order. Both the raw and the
effective key are scored (`handkey_raw` and `handkey`).
