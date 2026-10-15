# Adjudication of mse_anilallewar endpoints (contract M8b, T2; eval-m8b-pass1 finding 2)

Append-only. Blind adjudication of the model's `Endpoints` against the
pinned checkout, replayed by `msecend::apply_overrides`.

## 2026-10-15: first adjudication

- `/comments` is a bare class prefix: `CommentsController.java:30`
  (`@RequestMapping("/comments")`), sole handler `/{taskId}` (:59).
- `/{taskId}` (`TaskController.java:48`, class `/` at :22) and `/{userName}`
  (`UserController.java:51`, class `/` at :23) are REAL: Spring joins a
  method value with no leading slash under the class path. The model is
  right; rb misses them (real recall gap, kept as fn).

- REMOVE `ENDPOINT ANY /comments @comments-webservice` — bare class prefix
