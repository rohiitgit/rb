# Adjudication of mse_rohitghatol endpoints (contract M8b, T2; eval-m8b-pass1 finding 2)

Append-only. Blind adjudication of the model's `Endpoints` against the
pinned checkout, replayed by `msecend::apply_overrides`. Same app as
mse_anilallewar (fork); same mechanism with shifted lines.

## 2026-10-15: first adjudication

- `/comments` is a bare class prefix: `CommentsController.java:28`, sole
  handler `/{taskId}` (:55).
- `/{taskId}` (`TaskController.java:48`, class `/` at :22) and `/{userName}`
  (`UserController.java:47`, class `/` at :21) are REAL (no-leading-slash
  join). Kept as fn.

- REMOVE `ENDPOINT ANY /comments @comments-webservice` — bare class prefix
