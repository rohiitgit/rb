# Adjudication of mse_fernandoabcampos endpoints (contract M8b, T2; eval-m8b-pass1 finding 2)

Append-only. Blind adjudication of the model's `Endpoints` against the
pinned checkout, replayed by `msecend::apply_overrides`.

## 2026-10-15: first adjudication (class prefixes and Feign clients)

- Bare class prefixes with no handler: `CardServiceController.java:18`
  (`/api`), `StatementServiceController.java:19` (`/api`),
  `CardStatementServiceController.java:20` (`/api`); the composite's only
  real handler is `/api/statement-by-card` (:30).
- Six Feign-client paths credited to `card-statement-composite` are client
  interfaces served by card-service / statement-service, not by the
  composite: `service/CardService.java:18,21,24`
  (`/api/cards`, `/api/card/{}`, `/api/new-card`) and
  `service/StatementService.java:17,20,23`
  (`/api/statements`, `/api/statement/{}`, `/api/statement`), consumed via
  `@FeignClient(name="card-service")` / `@FeignClient(name="statement-service")`
  (`CardClient.java:5-6`, `StatementClient.java:5-6`).

- REMOVE `ENDPOINT ANY /api @card-service` — class prefix, no bare handler
- REMOVE `ENDPOINT ANY /api @statement-service` — class prefix, no bare handler
- REMOVE `ENDPOINT ANY /api @card-statement-composite` — class prefix, no bare handler
- REMOVE `ENDPOINT ANY /api/cards @card-statement-composite` — Feign client of card-service
- REMOVE `ENDPOINT ANY /api/card/{} @card-statement-composite` — Feign client of card-service
- REMOVE `ENDPOINT ANY /api/new-card @card-statement-composite` — Feign client of card-service
- REMOVE `ENDPOINT ANY /api/statements @card-statement-composite` — Feign client of statement-service
- REMOVE `ENDPOINT ANY /api/statement/{} @card-statement-composite` — Feign client of statement-service
- REMOVE `ENDPOINT ANY /api/statement @card-statement-composite` — Feign client of statement-service
