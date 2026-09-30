# Changelog

## [2026-09-30 19:00]
- M3a evaluator pass 2 fixes (contract Amendment 2):
  - **rb-syntax:** `Binding::visible`, the extent over which a declaration is in effect, computed from per-language scope kinds; all declaration forms recorded (loops, `with`/`except`/`catch`, comprehensions, destructuring, walrus, lambda parameters, Go multi-assignment and type switches, Java enhanced-for, resources and `instanceof` patterns).
  - **Resolver:** lookups by extent through a nested-interval tree (`Locals`), replacing position order; a definition's own name is not a self-reference; Java nested classes through imports.
  - **Modules:** Python source-root rule (importer's root, `""` or `src`).
  - **rb callers:** recursive targets list themselves once.
- Re-measured: petclinic 39.2% (was 37.8%), hono 14.8%, gin 63.4% (was 64.7%; its 207 self-references from type declarations are gone, 3 genuine ones remain), requests 30.4%.
- Files affected: crates/rb-syntax/src/{bindings,lib}.rs, crates/rb-indexer/src/{resolve,modules}.rs, crates/rb-store/src/code.rs, tasks/contract-m3a.md.

## [2026-09-30 16:00]
- M3a evaluator pass 1 fixes (12 defects; contract Amendment 1):
  - **Linear resolution:** containers are deduplicated; bindings and same-package lookups are indexed.
  - **Untyped declarations** are recorded, hide outer bindings, and never fall back to same-named types.
  - **Rebinding** uses the nearest preceding declaration.
  - **Class scope:** a class body is not a scope for its methods; Python class attributes are fields.
  - **Order and packages:** Java lookup order follows JLS §6.4.1; Go `_test` files are excluded from imported packages; Python absolute imports only match source-root modules.
  - **Self-calls** are kept.
  - **`rb impact`** includes dispatch callers.
  - **`rb status`** classifies sites by target count.
- Re-measured: petclinic 37.8%, hono 14.8%, requests 30.4% unchanged. gin fell from 70.3% to 64.7%: untyped `c, _ := CreateTestContext(…)` no longer borrows the type of a later anonymous handler's `c *Context` parameter.
- Files affected: crates/rb-syntax/src/bindings.rs, crates/rb-indexer/src/{resolve,modules}.rs, crates/rb-store/src/{code,query}.rs, crates/rb-cli/tests/{e2e.rs,golden/java-spring-jpa/impact-default-authenticate.txt}, tasks/contract-m3a.md.

## [2026-09-30 13:00]
- M3a R12 measured on spring-petclinic, hono, gin and requests; 80 of 80 sampled edges correct.
- Fixed: type resolution recursed without bound through self-referential bindings (`session = session.Session()`), overflowing the stack on psf/requests. `Index::resolve_type` now has an in-progress guard and a depth cap of 64, with a test.
- Files affected: crates/rb-indexer/src/resolve.rs, IMPLEMENT.md, tasks/lessons.md.

## [2026-09-30 12:00]
- M3a step 4: the code questions.
  - **`rb_store::code`** holds pure functions over the snapshot graph.
    - `resolve_target` (§9.1): exact key, `path:line`, key suffix, dotted name, plain name. It returns every candidate in the best tier and never picks silently.
    - `callers` (Q1): breadth-first, with CHA dispatch through `IMPLEMENTS`; the path certainty is the minimum along the path; depth 1–25; references on request.
    - `impact` (Q2): the target plus its `CONTAINS` members; tier 1 is requiring edges, tier 2 is inbound reachability with dispatch; roll-ups by service (own vs other) and owner; capped at 200,000 nodes, with `truncated`.
    - `Describer`: a display name, file:line and service per node.
  - **CLI:** `rb callers <target> [--depth N] [--include-references] [--min-certainty C] [--pick N]` and `rb impact <target> [--min-certainty C] [--pick N]`.
  - **Fixtures** gain the must-not-exist cases `LegacyAuth#authenticate` and `Cache#findUserByEmail`.
  - **e2e:** `code_question_goldens` covers all three fixtures, plus candidate listing for an ambiguous name.
- Files affected: crates/rb-store/src/{lib,code}.rs, crates/rb-cli/src/{main,code}.rs, crates/rb-cli/tests/{e2e.rs,golden/*}, fixtures/*.

## [2026-09-30 10:30]
- M3a steps 2–3: Tier B resolution in the indexer.
  - **`modules.rs`** resolves imports to snapshot files:
    - TS/JS relative specifiers, with extension and `index` probing, and `.js` naming `.ts`;
    - Java classes, wildcards, static imports and nested classes;
    - Python modules by suffix, with a shared-prefix tie-break, relative levels and submodules;
    - Go import paths, through each `go.mod`'s module path (new `Content::go_modules`).
  - **`resolve.rs`** emits `CALLS`, `REFERENCES` and `IMPLEMENTS` (provenance `heuristic`).
    - Resolution levels, per §5.4: lexical scope, the same file, imports, the same package.
    - Receivers: typed receivers, `this`/`self`/`super`, class-qualified and fully qualified calls, and module aliases.
    - Inheritance is searched breadth-first, capped at 64 levels. Several candidates are `ambiguous` with `candidates`, and an unknown receiver is never linked by name.
    - `IMPLEMENTS` covers type → supertype and method → overridden method.
  - **Coverage:** per file, `unresolved_references` counts unresolved calls and types and external references; Go and Python built-ins are skipped. `rb status` prints the resolved share.
  - **rb-syntax:** type references keep their qualifier (`store` in `store.DB`).
  - **e2e:** the M1b "no CALLS" guard is replaced by M3a's certainty invariant; the polyglot dump golden gains 3 verified `CALLS` edges; there is a new `status-code.txt` golden.
- Files affected: crates/rb-indexer/src/{lib,modules,resolve}.rs, crates/rb-git/src/boundaries.rs, crates/rb-syntax/src/lib.rs, crates/rb-store/src/query.rs, crates/rb-cli/{src/main.rs,tests/e2e.rs,tests/golden/polyglot-monorepo/*}, tasks/contract-m3a.md.

## [2026-09-30 08:30]
- M3a step 1: `rb-syntax` extracts more facts per file.
  - **`imports`:** module as written, named imports with local names, module alias, wildcard and static flags. Covers ES imports, Java single-type, wildcard and static imports, Python `import`/`from` with relative levels, and Go specs.
  - **`package`:** the Java or Go package clause.
  - **`bindings`:** names with a known type (fields, parameters, typed locals, constructor initialisers, TS parameter properties, `this.x`/`self.x` members, Go struct fields and composite literals). Go and Python built-in types are dropped. Members are scoped to their class.
  - The new modules are part of the extractor fingerprint, so cached facts are rebuilt.
- Files affected: crates/rb-syntax/src/{lib,imports,bindings}.rs, tasks/contract-m3a.md.

## [2026-09-30 07:00]
- M2b evaluator pass 7 fixes:
  - **`first_along`** re-examines a base after resolving it, so results no longer depend on source order.
  - **Django `Meta`:** a class without its own `Meta` inherits only its first abstract parent's `Meta`.
  - **Tests:** shadowed-order proxies and `Meta` chains, merged-Meta across parents, and a reversed 20,000-deep proxy chain.
- Files affected: crates/rb-frameworks/src/lib.rs, tasks/contract-m2b.md, docs §7.4.

## [2026-09-30 06:00]
- M2b evaluator pass 6 fixes:
  - **`first_along`:** a new iterative, memoised resolver. `PyClasses` now resolves every class's Django table (`resolve_django_tables`) and inherited `Meta` values (`inherited_meta`) with it, once per file. It replaces the recursive `django_table` and the per-class `Meta` walks, so there is no recursion depth and cost is linear.
  - **Flask:** detection requires `db.Model` or a name ending in `.db.Model`.
  - **Contract:** Amendment 6 records E2, E3, enums/records and nested Python bases as limitations for M3.
- Files affected: crates/rb-frameworks/src/lib.rs, tasks/contract-m2b.md.

## [2026-09-30 04:30]
- M2b evaluator pass 5 fixes:
  - **`BaseResolver`** tracks which names on the current path a result was computed without, and caches only complete results, so bindings no longer depend on interface names. There is a step budget of 100,000.
  - **Python mapping** is rebuilt around `PyClasses`: per-file base and `Meta` indexes, Django and Flask membership by reverse BFS, and an iterative `Meta` inheritance walk. `reaches`, `local_bases`, `django_is_model`, `own_meta` and `inherited_meta_from` are removed.
  - **`mappings`** only considers class definitions.
- Files affected: crates/rb-frameworks/src/lib.rs, tasks/contract-m2b.md.

## [2026-09-30 03:00]
- M2b evaluator pass 4 fixes:
  - **Repository bases:** `repository_bases` was a round-based fixpoint that could loop forever on same-named interface cycles. It is now a memoised per-name resolver (`BaseResolver`) with a cycle guard and a depth cap of 512. Every declaration of a name must agree, and `org.springframework.data.…` supertypes bind the built-ins directly.
  - **Hole filter:** only non-literal native SQL has `$n` names filtered.
  - **Python walks:** they use visited sets (`reaches`, `inherited_meta_from`, `django_table` with `seen`). `MAX_MODEL_DEPTH` is removed.
  - **Flask:** models under `__abstract__` bases.
  - **Tests:** a cycle, a Spring-name clash, a wide Python hierarchy (40 classes), a 3000-level repository chain, Flask abstract bases, and `$` in static SQL.
- Files affected: crates/rb-frameworks/src/lib.rs, crates/rb-indexer/src/orm.rs, docs §7.4, tasks/contract-m2b.md.

## [2026-09-30 01:10]
- M2b evaluator pass 3 fixes (3 contract violations, and the lower-severity defects that were cheap to fix):
  - **rb-syntax:** `Supertype::implements`.
  - **rb-frameworks:**
    - `Target::Sql { text, literal }`.
    - Repository-base bindings are re-derived every round, so late clashes conflict. The redundant removal loop, which a mutation check showed was dead code, is removed.
    - Flask `__abstract__`; SQLAlchemy `__tablename__ = None`.
    - Django: schema-quoted `db_table`, `Meta(Base.Meta)` and whole-Meta inheritance, nested `models` app labels.
    - JPQL holes, SpEL and `UPDATE VERSIONED`.
    - Prisma attributes sharing a line.
  - **Indexer:** inheritance follows only `extends`. Dynamic native SQL never names hole tables, and its static tables are heuristic.
  - **Contract:** the O5 diagnostic name is corrected; Amendment 3 lists the fixes and the accepted known gaps.
- Files affected: crates/rb-syntax/src/{lib,decorations}.rs, crates/rb-frameworks/src/lib.rs, crates/rb-indexer/src/orm.rs, docs §7.4, tasks/contract-m2b.md.

## [2026-09-29 23:40]
- M2b evaluator pass 2 fixes (11 new code defects; the 13 earlier ones verified fixed), test-first:
  - **rb-syntax:** Python class constants keep non-string values, marked non-literal. String values unwrap parentheses.
  - **rb-frameworks:**
    - `Mapping::table` is `Result<TableName, Unmapped>`, with reasons `DynamicName` and `UnknownProxyParent`. The `superclass` field is removed.
    - Spring Data targets are `Target::EntityClass`.
    - JPQL uses flattened non-literal text and skips comments.
    - Conflicting generic repository bases are dropped.
    - A snake-cased JPA schema is flagged assumed.
    - Django: only `models.Model`; `app_label` inherited from abstract bases; `Meta.proxy` marks a model.
    - New `flask_snake_case`. `prisma_root` is removed.
  - **Indexer:**
    - JPA inheritance walks all Java classes (through `@MappedSuperclass`) and prefers a same-directory superclass.
    - A class-name index for repositories.
    - Prisma resolves by the longest shared directory prefix.
    - New diagnostic `orm_unresolved_proxy_parent`.
- Files affected: crates/rb-syntax/src/{lib,strings,decorations}.rs, crates/rb-frameworks/src/lib.rs, crates/rb-indexer/src/orm.rs, docs §7.4, tasks/contract-m2b.md.

## [2026-09-29 22:30]
- M2b evaluator fixes (13 defects), test-first:
  - **rb-syntax:** decoration arguments and class constants carry `literal`; concatenated literals are flattened; `true`/`false` class constants are kept; definitions record `type_params`.
  - **rb-frameworks:**
    - JPA naming follows Hibernate's `PhysicalNamingStrategySnakeCaseImpl` (`hibernate_physical_name`) for explicit names too, with quoted names kept. This corrects the earlier claim that `snake_case` matched Hibernate: it matches TypeORM and Flask-SQLAlchemy only.
    - Dynamic names give no table.
    - Django: abstract, proxy, `app_label`, same-file subclasses and multi-table inheritance. `db.Model` is Flask-SQLAlchemy.
    - JPQL counts only clause-level FROM/JOIN.
    - Generic base repositories (`repository_bases`).
    - The Prisma receiver must look like a client.
    - `@@map(name:)`; `prisma_root`.
  - **Indexer:**
    - `add_mappings` resolves the whole snapshot at once: JPA SINGLE_TABLE subclasses map to the root's table, and Prisma accessors resolve to the nearest schema (or `ambiguous`).
    - `MAPS_TO` edges are aggregated per (symbol, table), so a class declared twice no longer fails the index.
    - New diagnostics: `orm_dynamic_table_name`, and `unparsed_sql` (callee `Query`) for native queries.
- Files affected: crates/rb-syntax/src/{lib,keys,strings,decorations}.rs, crates/rb-frameworks/src/lib.rs, crates/rb-indexer/src/{lib,orm,sql}.rs, docs/repository-brain-design.md §7.4, tasks/contract-m2b.md.

## [2026-09-29 20:10]
- M2b step 3: the indexer emits ORM edges.
  - **`MAPS_TO`** (class or Prisma model → table, provenance `orm`): exact for explicit names, synthetic when the name comes from a naming rule. JPA names are folded to lowercase, as Hibernate emits them unquoted.
  - **ORM `QUERIES`:** native `@Query` is exact, conventions are synthetic, and JPQL is heuristic. Entities resolve across files, and duplicate entity names give `ambiguous`.
  - **Diagnostics:** `orm_unresolved_entity`.
  - **Prisma models** become symbols defined in `schema.prisma`.
  - **`assemble`** now takes a `Content` struct (facts, `.sql`, `.prisma`).
  - **`rb table-deps`** shows "Mapped by".
  - New goldens for the JPA and Prisma fixtures.
- Files affected: crates/rb-indexer/src/{lib,orm,sql,syntax}.rs, crates/rb-sql/src/lib.rs, crates/rb-syntax/src/{lib,keys}.rs, crates/rb-store/src/query.rs, crates/rb-cli/{src/main.rs,tests/e2e.rs}, goldens.

## [2026-09-29 19:20]
- M2b step 2: new `rb-frameworks` crate. Pure packs over syntax facts cover:
  - JPA `@Entity`/`@Table` (by default the table is the entity name snake-cased, flagged assumed);
  - Spring Data repositories: inherited CRUD, derived queries, native and JPQL `@Query`;
  - a JPQL parser (association paths skipped, FROM lists with joins);
  - Prisma schema models and client calls;
  - TypeORM `@Entity`;
  - SQLAlchemy `__tablename__`;
  - Django models (`Meta.db_table` or `<app>_<model>`).
- The `snake_case` implementation follows TypeORM's `DefaultNamingStrategy` and Hibernate's `PhysicalNamingStrategySnakeCaseImpl`, which Spring Boot uses by default. Both were checked against their sources.
- `rb-syntax`: call references record their receiver `qualifier`.
- Files affected: crates/rb-frameworks/*, crates/rb-syntax/src/lib.rs, Cargo.toml.

## [2026-09-29 18:30]
- M2b step 1: `rb-syntax` extracts framework-neutral facts per definition:
  - decorations: Java annotations, and Python/TypeScript decorators (including the siblings before a method and those on an export statement), with decoded literal arguments; TypeORM-style option objects are flattened into keyed arguments;
  - supertypes, with type arguments;
  - Python class-body string constants.
- The new source file is part of the cache fingerprint.
- Files affected: crates/rb-syntax/src/{lib,decorations,strings,keys}.rs.

## [2026-09-29 17:55]
- M2a, fourth evaluator pass (4 defects, 0 regressions); all fixed test-first and covered by mutation checks.
  - **Lexer errors** keep the statements before the failure point, taken from the lexer's "at or near" text.
  - **`pg_dump` `COPY … FROM stdin` data** is blanked, so it's no longer lexed as SQL. This removes the phantom `DROP`/`DELETE` edges and the whole-file errors.
  - **Placeholders glued to arithmetic** (`bal-?`, `n+?`) are rewritten.
  - **Multi-action `CREATE RULE (…; …)`** stays one statement.
- Files affected: crates/rb-sql/src/lib.rs, docs/repository-brain-design.md.

## [2026-09-29 17:20]
- M2a, third evaluator pass (6 defects, including a panic); all fixed test-first, and mutation checks confirm each fix is covered by a test.
  - **Splitting** is now done on Postgres lexer tokens alone. This removes the parser/scanner/coverage-check combination and the psql blanking pass.
    - Range start can no longer exceed its end (U+00A0).
    - Non-keyword statements (`USE`, `GO`, garbage) no longer hide the others.
    - `\gset` terminates a statement, and `\copy` is kept as `COPY`.
    - `BEGIN ATOMIC` bodies are tracked by keyword depth.
  - **Placeholders** glued to comparisons are rewritten (`id=?`, `>=?`, `=%(id)s`).
  - **Tree walk:** `FOR UPDATE OF alias` is no longer a table. `UNION` arms' `WITH` clauses are scoped. `PARTITION OF`/`INHERITS` parents, `CREATE FOREIGN TABLE` and `ALTER TABLE SET SCHEMA` are handled.
- Files affected: crates/rb-sql/src/lib.rs, docs/repository-brain-design.md.

## [2026-09-29 16:30]
- M2a, second evaluator pass (9 issues; all fixed test-first, plus one hidden bug found by a new test): `rb-sql` core rebuilt on Postgres' own tools. My hand-written splitter and character scanner are deleted.
  - **Splitting:** statements are split by `split_with_parser`, falling back to `split_with_scanner`. This fixes `BEGIN ATOMIC` bodies and `$` in identifiers.
  - **Coverage check:** if tokens are left uncovered, the whole text is reported. This fixes the lexer silently dropping text with unbalanced parentheses.
  - **Placeholders and meta-commands:** both are handled as lexer tokens. Comments and `E''` strings are safe, and meta-commands work anywhere on a line.
  - **Tables:** found by a full JSON walk of the protobuf tree, with proper CTE scoping. Nested scopes and sibling order fixed; `TABLESAMPLE`, `RETURNING`/`ON CONFLICT`/`MERGE` subqueries, `PREPARE`, cursors, `CREATE RULE` and `RENAME CONSTRAINT` now handled.
  - **Table keys:** parts containing `.` or `"` are quoted.
  - **Diagnostics:** one `unparsed_sql` per sink call site, with `failed_statements`.
- Mutation checks confirm each fix is covered by a test.
- Files affected: crates/rb-sql/{Cargo.toml,src/lib.rs}, crates/rb-indexer/src/sql.rs, docs/repository-brain-design.md, goldens.

## [2026-09-29 15:40]
- M2a fixes after the evaluator pass and the S16 measurement, each test-first.
- `rb-sql` rewritten:
  - **Own statement splitter:** handles quotes, `E''`, `$tag$`, nested comments and `psql` meta-commands.
  - **Per-statement parse:** one bad statement no longer hides the others, and ranges always refer to the original text. This fixes a crash.
  - **Parse-tree walk:** handles CTE shadowing, nested DML operations, dotted quoted names, `COPY` direction, `RENAME`, `SELECT INTO`, and `ALTER TYPE` as not a table. Unqualified `pg_*` names are system catalogs.
  - **Token-bounded placeholders:** covers `?`, `:name`, `%s` and `%(name)s`.
- `rb-syntax`: `\x`, `\u`, `\u{}`, `\U` and octal escapes are decoded, and keywords must be whole words.
- Indexer:
  - a BOM in `.sql` files is handled with raw-byte spans;
  - each bad statement becomes its own diagnostic;
  - `createQuery` (JPQL) is skipped and `createNativeQuery` is a sink.
- Store: distinct-site SQL coverage with an honest label, and case-insensitive table lookup.
- Files affected: crates/rb-sql/src/lib.rs, crates/rb-syntax/src/strings.rs, crates/rb-indexer/src/sql.rs, crates/rb-store/src/query.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/e2e.rs, docs/repository-brain-design.md, tasks/contract-m2a.md, IMPLEMENT.md, README.md.

## [2026-09-29 14:20]
- M2a step 4: `rb table-deps <table>` (text or `--json`).
  - It lists migrations and accessing code grouped by service, with ops, certainty, schema assumption and file:line.
  - A bare name that matches several schemas lists the candidates and exits non-zero.
  - `rb status` shows SQL coverage (tables, query sites, migration statements, unparsed), derived from the snapshot.
  - The polyglot fixture gained Java `JdbcTemplate`, Node `pg`, Go `database/sql` and migrations.
  - New goldens: table-deps (3), status-sql.
- Files affected: crates/rb-store/src/{lib,query}.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/e2e.rs, crates/rb-model/src/lib.rs, fixtures/polyglot-monorepo/*, crates/rb-cli/tests/golden/polyglot-monorepo/*.

## [2026-09-29 13:40]
- M2a step 3: the indexer adds `Table` nodes.
  - `MIGRATES` (File → Table, provenance `sql_file`) for every statement in `.sql` files.
  - `QUERIES` (Symbol or File → Table, provenance `sql_literal`) for SQL candidates. They are aggregated per (source, table), with the union of `ops`, the best certainty, and every site as evidence. Sink calls give `exact`; other candidates give `heuristic`.
  - `unparsed_sql` is reported only for unparseable `.sql` files and sink arguments.
  - Statement ranges now start at the first token, skipping leading comments.
  - New provenance: `sql_file`.
- Files affected: crates/rb-indexer/src/{lib,sql}.rs, crates/rb-sql/src/lib.rs, crates/rb-model/src/lib.rs, crates/rb-cli/tests/golden/polyglot-monorepo/dump.jsonl.

## [2026-09-29 13:05]
- M2a step 2: `rb-syntax` extracts SQL candidates, meaning SQL-looking string expressions. They are cached in `FileFacts.sql` with the normalised text, span, enclosing definition and callee name.
  - Covers TS/JS template holes and tagged templates, Java text blocks (dedented), Python f-strings and implicit concatenation, Go raw strings, and `+` concatenation.
  - Escapes are decoded.
  - Annotations and decorators are skipped.
  - `rb-sql` retries JDBC `?` and named `:param` placeholders as `$n`.
- Files affected: crates/rb-syntax/src/{lib,strings}.rs, crates/rb-sql/src/lib.rs, tasks/contract-m2a.md.

## [2026-09-29 12:20]
- M2a step 1: new `rb-sql` crate. It uses `pg_query` 6.2 (the real PG17 parser) to split SQL into statements and report each table with its operations: read, insert, update, delete, merge, create, alter, drop, truncate.
  - CTE names are excluded and system schemas are ignored.
  - Unqualified names resolve to `public` and are flagged `schema_assumed`.
  - Parse errors are returned, never guessed.
- Files affected: crates/rb-sql/*, Cargo.toml, tasks/contract-m2a.md.

## [2026-09-29 11:30]
- M1b review fixes, each test-first:
  - a Go method whose receiver type is declared in its own body crashed the process (a parent cycle). Methods now attach only to top-level types, and key building is iterative and cycle-safe;
  - a Go method no longer attaches to a same-named type local to another function;
  - nesting is capped at 64 levels (`definitions_too_deep` diagnostic), so key size stays linear;
  - TypeScript `namespace`, type aliases and enums are now definitions;
  - definitions inside anonymous functions (callbacks, lambdas, anonymous classes, Go func literals) are locals, not symbols. Previously unrelated test-local types merged into one symbol.
- Cache correctness: the facts cache key is now `rb_syntax::extractor_id()`, which combines the grammar versions with a fingerprint of the bundled queries and the extractor source, so stale facts can't be served after an edit.
- C18 moved to public repos (spring-petclinic, hono, gin, requests). The two private repos previously used for C18 were deleted from the Brain, and their references removed from IMPLEMENT.md and the M1b contract.
- Files affected: crates/rb-syntax/src/{lib,keys,queries}.rs, crates/rb-indexer/src/syntax.rs, crates/rb-cli/tests/e2e.rs, IMPLEMENT.md, tasks/contract-m1b.md, docs/repository-brain-design.md, README.md.

## [2026-09-29 10:25]
- M1b step 3: syntax facts are cached in `rb.file_facts_cache`, keyed by blob plus extractor version and language, through `Store::load_facts`/`save_facts` and a CLI `StoreCache` adapter.
  - `rb index` reports `parsed N source files`.
  - New `rb index --no-cache` bypasses the cache.
  - The index subcommand's arguments are now an `IndexArgs` struct.
  - New e2e test: a warm cache parses 0 files and dumps byte-identically to a cold run.
- Files affected: crates/rb-store/src/lib.rs, crates/rb-cli/src/main.rs, crates/rb-cli/Cargo.toml, crates/rb-cli/tests/e2e.rs.

## [2026-09-29 10:05]
- M1b step 2: the indexer turns definitions into `Symbol` nodes with `DEFINED_IN` (evidence: the name span) and `CONTAINS` edges.
  - Files are chosen for parsing by language, binary/generated flags and a 1 MiB cap (over the cap gives an `extraction_skipped` diagnostic).
  - Parsing runs in parallel on `std::thread::scope`, in batches of 512.
  - Syntax errors become diagnostics.
  - A `FactsCache` trait keeps the indexer I/O-free.
- The pipeline version is now `m1b-syntax-1`. New golden: `find-login-symbols.txt`. The dump golden includes symbols.
- Files affected: crates/rb-indexer/src/{lib,syntax}.rs, crates/rb-syntax/src/lib.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/e2e.rs, crates/rb-cli/tests/golden/polyglot-monorepo/*.

## [2026-09-29 09:40]
- M1b step 1: new `rb-syntax` crate. It parses TypeScript, TSX, JavaScript, Java, Python and Go with Tree-sitter 0.27 and extracts definitions (with parents) and references (with their enclosing definition). It also derives heuristic symbol keys in SCIP shape.
  - Java gets an extra query for enums, records and constructors.
  - TS/TSX run both the JS and TS tag queries.
  - Go methods attach to their receiver type.
  - 15 tests cover these, and mutation checks confirm they catch regressions.
- Files affected: crates/rb-syntax/*, Cargo.toml.

## [2026-09-29 08:55]
- Engineer review of M1a:
  - service `role` (service, library, unassigned) replaces the deployable and synthetic flags;
  - stored `path` and owner `type` removed (both derivable);
  - `project_for` uses binary search, with sorting enforced by `Boundaries::new`;
  - explicit `rb.toml` name override.
- Evaluator fixes, each with a regression test:
  - `docs/*/` and `*/` now match;
  - a literal `*` in a filename no longer breaks wildcard matching;
  - a directory named `(root)` can no longer capture unassigned files (unassigned files now live on `.`);
  - CRLF and BOM handled in CODEOWNERS;
  - BOM handled in manifests;
  - non-UTF-8 paths are skipped instead of aborting the run;
  - `/* c */ package main` is detected;
  - all text ordering is bytewise (`COLLATE "C"`), not locale-dependent;
  - owners are lowercased;
  - `rb find --limit` is range-checked;
  - `pom.xml` with a DOCTYPE parses.
- Files affected: crates/rb-git/src/{glob,codeowners,boundaries,lib}.rs, crates/rb-indexer/src/lib.rs, crates/rb-model/src/{lib,manifest}.rs, crates/rb-store/src/{lib,query}.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/e2e.rs, docs/repository-brain-design.md.

## [2026-09-29 08:10]
- M1a structure: CODEOWNERS owners with evidence, service boundary detection with `rb.toml`, and the `rb find` and `rb service` commands. Edges now carry evidence, and diagnostics keep their path (migration 0002).
- Fixed pg_trgm being installed into AGE's `ag_catalog` schema (migration 0003). Migrations now pin `search_path = public`.
- Files affected: crates/rb-git/src/{glob,codeowners,boundaries,lib}.rs, crates/rb-indexer/*, crates/rb-store/src/{lib,query}.rs, crates/rb-model/src/*, crates/rb-cli/*, migrations/0002-0003, fixtures/polyglot-monorepo.

## [2026-09-29 07:25]
- Initial commit a8f5ac2: the design doc and M0 foundations. This covers the Rust workspace, Postgres 18 + AGE 1.8.0 in compose, snapshot publishing in one transaction, and the `rb migrate`, `index`, `status`, `dump` and `cypher` commands.
