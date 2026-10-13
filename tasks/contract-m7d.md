# M7d contract: DI and SQL roles (stress batch C, part 2) (2026-10-13)

**Status:** S1, S2, S4, S5 implemented 2026-10-13; S3 deferred (see amendment 1). Stacked on M7c (`8078ac2`).

## Why

| Finding | Repo | What the stress campaign found |
|---|---|---|
| F28 | eShopOnWeb (`4da8212`) | .NET DI registrations are dropped wherever the DI namespace arrives through implicit or global usings (the .NET 6+ default). Classes registered behind an interface never become injection owners. Open generics are skipped. |
| F26 | nestjs-realworld (`c1c2cc4`) | `@InjectRepository(Entity)` points report `no_bean` though TypeORM provides them. |
| F23 | fastapi-realworld (`029eb77`) | Every `.sql` file is a migration, so aiosql query files become `MIGRATES`. |

## Claims

| # | Claim | Mechanism | Residue |
|---|---|---|---|
| S1 (F28a) | **A C# file imports DI implicitly when its project does.** The nearest `.csproj` has `Sdk` `Microsoft.NET.Sdk.Web`, `.Worker` or `.BlazorWebAssembly` with `<ImplicitUsings>enable|true</ImplicitUsings>`, or any file under that project has `global using Microsoft.Extensions.DependencyInjection;`. | `dotnet_di(facts, implicit_di)`; computed in `rb-indexer/src/di.rs` from `Inputs.manifests` and file facts | `Directory.Build.props` and `<Using Include>` items stay unread (recorded) |
| S2 (F28b) | **A class registered as an implementation is a registered owner.** `AddScoped<IX, X>` registers `X` (bean `created`), so `X`'s constructor parameters are points. | `rb-indexer/src/di.rs` `resolve_lane_point` owner check | — |
| S3 (F28c) | **Open-generic registrations `AddX(typeof(IR<>), typeof(R<>))` are beans** of the open service type, created by the implementation. | `dotnet_di` reads two `typeof` arguments | Single-`typeof` and factory forms stay unread |
| S4 (F26) | **`@InjectRepository(E) x: Repository<E>` is external** (TypeORM-provided), not `no_bean`. A point whose type is imported from outside the repository and that no bean serves is bucketed `external`, as in the Spring lane. | `rb-indexer/src/di.rs` Nest/.NET lane | — |
| S5 (F23) | **A `.sql` file is a migration only by evidence of one.** It sits under a migrations directory (`migrations/`, `migration/`, `db/migrate/`, `flyway`/`liquibase` conventions, `V<n>__*.sql`, numbered prefixes), or it holds DDL and no `-- name:` query markers. Otherwise its statements are code sites (`QUERIES`) of the file. | `rb-indexer/src/sql.rs` `add_migrations` | Linking aiosql query names to their Python callers stays out (recorded) |

## Verification

- Tests first per claim, with a mutant per rule.
- fmt, clippy, and workspace tests with the DB and real Docker: 0 failures, no skips.
- Invariant against M7c on all targets, with every diff classified (eShopOnWeb, nestjs-realworld and fastapi-realworld are expected to change).
- `PIPELINE` bumped to `m7d-1`.

## Progress (amendment 1, 2026-10-13)

| Claim | Done | Evidence |
|---|---|---|
| S1 (F28a) | `Import.global` (C# `global using`); `.csproj` loaded as a manifest; `DotnetImplicit`: a project is on if its SDK is Web, Worker or BlazorWebAssembly with `ImplicitUsings` enable/true, or if any of its files has a global DI using. `dotnet_di(facts, implicit_di)`. | Mutants m40, m41, m42, m44 (the implicit-off negative was added after m44 survived) and m45 killed. |
| S2 (F28b) | The lane's owner check also accepts a bean's `created` (implementation) types. | Mutant m43 killed. eShopOnWeb: 5 new `INJECTS` from implementation-registered owners. |
| S3 (F28c) | **Deferred.** C# `typeof(X)` arguments are `Arg::Other` without text; reading them needs an extractor change. `IRepository<>`/`IAppLogger<>` points stay unresolved. | Residue |
| S4 (F26) | A lane point with no qualifier, no repository type, and a type imported (by name) from outside the repository is `external`, with no `di_unresolved`. | Mutants m46/m47 killed. nestjs-realworld: 8 `no_bean` became `external`. |
| S5 (F23) | `add_sql_file` + `is_migration`: `-- name:` (aiosql/yesql, exact case, not pg_dump's `-- Name: …; Type:`) or `-- :name` (HugSQL) gives queries; migration path conventions give migration; else DDL (or nothing parsed) gives migration, otherwise `QUERIES` from the file. | Mutants m48–m55 killed (m49 and m55 needed marker- or dir-only cases). fastapi-realworld: 13 aiosql rows `MIGRATES`→`QUERIES`. |
| (found) | C# constructor-body locals were taken as constructor points (constructors are no definitions in the C# facts). The C# `parameter` arm now records its position, and `dotnet_di` takes parameters only. This also drops eShopOnWeb's 2 `event Action` false points. | Mutants m56/m57 killed. |

**Measurement mistake caught by the invariant (first run):** the first `-- name:` rule was case-insensitive and matched pg_dump's `-- Name: x; Type: TABLE` comments, reclassifying 87 mattermost schema dumps as query files. It was fixed test-first and recorded in `tasks/lessons.md`.

**Verification (second run):**
- fmt and clippy clean.
- Workspace with DB and **real Docker** (`var/m7d/logs/p4-workspace2.log`): **995 passed, 0 failed, 0 skipped**.
- Invariant against M7c (`var/m7d/logs/invariant2.txt`, sha256 `4735c4d1…`): **26/31 identical**, all 18 fixtures included. Classified:
  - **fastapi-realworld:** 13 aiosql rows `MIGRATES`→`QUERIES`.
  - **mattermost:** 3 INSERT-only seed files in `testlib/testdata`, `MIGRATES`→`QUERIES` (DML-only, no migration path).
  - **nestjs-realworld:** −8 `no_bean`, now `external`.
  - **eShopOnWeb:** +5 `INJECTS` net (implementation owners; `CatalogItemService → HttpService` became ambiguous because Web and BlazorAdmin both register `HttpService`); −2 false `event Action` points; +17 `di_unresolved` newly visible on now-registered owners (open generics S3, framework types through namespace usings, `AddDbContext`), all residue.
  - **microservices-demo:** +3 `di_unresolved` on real constructor parameters of framework types (`IConfiguration`, `IDistributedCache`); the false local-variable points never reached the final binary.
- `PIPELINE` `m7d-1`.

**Residue:**
- S3 (open generics).
- Framework types arriving through C# namespace usings are not "imported from outside" by name, so they stay `unresolved`, not `external`.
- `AddDbContext`, factories and `Directory.Build.props` stay unread.
