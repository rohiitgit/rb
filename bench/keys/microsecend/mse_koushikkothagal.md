# Adjudication of mse_koushikkothagal endpoints (contract M8b, T2; eval-m8b-pass1 finding 2)

Append-only. Blind adjudication of the model's `Endpoints` against the
pinned checkout, replayed by `msecend::apply_overrides`.

## 2026-10-15: first adjudication (bare class prefixes)

- `CatalogResource.java:20` class `/catalog`, sole method `/{userId}` (:29)
- `MovieResource.java:13` class `/movies`, sole method `/{movieId}` (:22)
- `RatingsResource.java:10` class `/ratingsdata`, methods
  `/movies/{movieId}` (:13) and `/user/{userId}` (:18)

No bare handler exists on any of the three prefixes. rb found all 4 real
endpoints.

- REMOVE `ENDPOINT ANY /catalog @movie-catalog-service` — bare class prefix
- REMOVE `ENDPOINT ANY /movies @movie-info-service` — bare class prefix
- REMOVE `ENDPOINT ANY /ratingsdata @ratings-data-service` — bare class prefix
