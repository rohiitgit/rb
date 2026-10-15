# Adjudication of mse_mdeket endpoints (contract M8b, T2; eval-m8b-pass1 finding 2)

Append-only. Blind adjudication of the model's `Endpoints` against the
pinned checkout, replayed by `msecend::apply_overrides`.

## 2026-10-15: first adjudication

- `/recommendation` is a bare class prefix:
  `RecommendationController.java:23` (`@RequestMapping("/recommendation")`),
  all 6 handlers on sub-paths (:33, :42, :51, :56, :67, :72).
- `/newuser` is a Feign-client path: `recommendation-client/.../service/UserService.java:26`
  (`@FeignClient("user-service")`, `value="/newuser"`); `user-service`
  serves only `/user` and `/user/{userId}` (`UserController.java`).

rb found all 12 genuinely served endpoints.

- REMOVE `ENDPOINT ANY /recommendation @recommendation-service` — bare class prefix
- REMOVE `ENDPOINT ANY /newuser @user-service` — Feign client, not served here
