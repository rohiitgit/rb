# Adjudication of mse_piggymetrics endpoints (contract M8b, T2; eval-m8b-pass1 finding 1, B14)

Append-only. Each entry comes from a blind adjudication of the model's
`Endpoints` against the pinned checkout
(`var/bench/repos/mse_piggymetrics` at
`6bb2cf9ddbca980b664d3edbb6ff775d75369278`), replayed by
`msecend::apply_overrides`. `ENDPOINT ANY <path> @<service>`; paths
normalised (`{}` for params).

## 2026-10-15: first adjudication (context path from Spring Cloud Config)

The model lists unprefixed paths, but the services are served behind the
config server's shared `server.servlet.context-path`:

- `config/src/main/resources/shared/account-service.yml:19-21`
  `context-path: /accounts`
- `config/src/main/resources/shared/statistics-service.yml:19-21`
  `context-path: /statistics`
- `config/src/main/resources/shared/notification-service.yml:10-12`
  `context-path: /notifications`

Controller code has no compensating class prefix:

- `account-service/.../controller/AccountController.java:13-14` no class
  mapping; methods `/{name}` (:19-20), `/current` (:25, :30), `/` (:35)
- `statistics-service/.../controller/StatisticsController.java:15` no class
  mapping; methods `/current` (:20), `/{accountName}` (:26, :32)
- `notification-service/.../controller/RecipientController.java:15-16`
  class `/recipients`; methods `/current` (:21, :26)

So the 6 model entries below are wrong paths that rb also emits (6 spurious
tp under the contract's "full effective path" identity). They are removed;
the true prefixed paths are added (2 already in gold, 4 new and currently
fn for rb). `/recipients` is a bare class prefix (finding 2), removed too.

Blind verdicts (`var/bench/labels/mse_piggymetrics/verdicts.md`): S6–S9 and
the unprefixed-form readings of S1/S3/S4 are FALSE with the evidence above.
S1/S2 split on `/accounts/{}` and S4/S5 split on `/statistics/{}`: the FALSE
readings dispute the model's `{accountName}` param name (the code serves
`{name}`), which the contract's `{}` normalisation does not distinguish, and
the TRUE readings confirm the normalised endpoint is served. Both stay in
gold (currently fn: rb emits no prefixed path).

- REMOVE `ENDPOINT ANY /{} @account-service` — serves `/accounts/{name}`
- REMOVE `ENDPOINT ANY / @account-service` — serves `/accounts`
- REMOVE `ENDPOINT ANY /current @account-service` — serves `/accounts/current`
- REMOVE `ENDPOINT ANY /current @statistics-service` — serves `/statistics/current`
- REMOVE `ENDPOINT ANY /{} @statistics-service` — serves `/statistics/{accountName}`
- REMOVE `ENDPOINT ANY /recipients/current @notification-service` — serves `/notifications/recipients/current`
- REMOVE `ENDPOINT ANY /recipients @notification-service` — bare class prefix; only `/current` handlers
- ADD `ENDPOINT ANY /accounts @account-service` — prefixed account root
- ADD `ENDPOINT ANY /accounts/{} @account-service` — already in gold; no-op if present
- ADD `ENDPOINT ANY /accounts/current @account-service` — prefixed account current
- ADD `ENDPOINT ANY /statistics/current @statistics-service` — prefixed statistics current
- ADD `ENDPOINT ANY /statistics/{} @statistics-service` — already in gold; no-op if present
- ADD `ENDPOINT ANY /notifications/recipients/current @notification-service` — prefixed notification path
