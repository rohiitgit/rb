# M7d contract: DI and SQL roles (stress batch C, part 2) (2026-10-13)

**Status:** contract written 2026-10-13 from `tasks/stress-findings.md` batch C; not started. Stacked on M7c.

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
