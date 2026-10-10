# M6b evaluator pass 1 (full review)

- **Date:** 2026-10-10 (wall clock)
- **Commit at start:** `4983f62` (phase 9 close, clean tree); **end:** same + pass-1 fixes E1, E2 (uncommitted at pass close, committed with phase 11).
- **Environment:** Apple Silicon, Docker context `desktop-linux`, Rust via `/opt/homebrew/opt/rustup/bin`.
- **Database:** `postgresql://rb:rb@127.0.0.1:55433/rb`, 0 repos at start and end (phase-10 batch cleanup verified). No new repos created by this pass (all probes are unit-level).
- **Method:** adversarial battery against each S-claim — probes written to FAIL on violation (2 did: E1, E2, fixed test-first below); confirmations that must pass (4 did); prior phases' mutants, units and the phase-10 public-repo measurements taken as supporting evidence and re-checked where the fixes could move them (E1/E2 trigger grep over re-cloned express/ftgo/microservices/piggymetrics at their pins: no trigger shapes anywhere — §E1/E2 notes).

## Baseline (phase-9 close, taken as recorded; re-verified in pass 2)

fmt clean, clippy `-D warnings` clean; workspace units 730/0; store 66, server 12; e2e 32/32 no `skipping:`; api 1/1; no listener; goldens classified per hunk (phase 9 record).

## Criteria

| # | Verdict | Evidence |
|---|---|---|
| S1 facts | PASS | Phase-1 units + S9's `Init::Member` extraction unit (`typescript_destructured_member_inits`, failed first as compile error) and fold-hole unit (`destructured_members_stay_holes`, with a follow-vs-hole distinguishing row). No golden change from facts alone (fixture e2e byte-identical until S9 rows). |
| S2 gin groups | PASS | Amendment-3 units/mutants; phase-10 gin re-measurement re-derived here: 19 `?router=` endpoints out, 26 composed-path endpoints in, 26 `route_prefix_unknown` diagnostics out, prefix_unknown endpoints 19 → 0; sampled in source (`POST /api/users/login` through the nested `v1.Group("/api")` chain; prefix-less nodes from `unit_test.go`'s own root call sites — D1 per-site identity). |
| S3 express chains | PASS | Amendment-4 units/mutants (M-c/M-d/M-e biting); phase-10 express re-derived: 19 unknown keys → 19 composed `/api` keys, counts equal, status otherwise identical. |
| S4 in-process/empty | PASS | Amendment-5 units/mutants; phase-10 mattermost re-derived: 8 ambiguous CONSUMES (4 `_test.go` functions) removed, 39 unmatched + 4 dynamic → 53 in-process + 8 empty-url, every transition span-checked; sinks read in source (incl. production `imageproxy/local.go:129` "fake a request to it" and 8× `NewRequest("", …)` in `handlers_test.go`). Pass-1 confirmation: hand-rolled `ResponseWriter` impl stays unmarked (`go_in_process_hand_rolled_recorder_is_not_a_sink`, green first try). |
| S5 axios folding | PASS | Amendment-6 units/mutants; fixture `resendViaCfg`/`resendExternal` guards green; ftgo dynamics fold through it (see S6/S11 rows). |
| S6 members | PASS | Amendment-7 hole battery (double, non-ctor, update, alias, alias-only, subclass, assign-kill, unfoldable, field-init, cycles) + growth; ftgo's 4 dynamics read genuine in source (constructor `this.consumerService` single write; restaurant 3-way ambiguity over three real controllers). |
| S7 userinfo | PASS | Amendment-8 units/mutants; pass-1 confirmation: literal `@` with no hole keeps today's host (`a_hole_in_userinfo_keeps_the_literal_host` row, green first try). |
| S8 narrowing | PASS with fix E2 | Amendment-9 units/mutants; 0 lifts on fixtures + all six measurement repos (re-derived). Pass-1 confirmations: same-package type name lifts (`com.example.LoadBalancerClient`), non-binding import lifts — both green first try. **E2** below. |
| S9 breadth | PASS with fix E1 | Amendment-10 units (29 mutants killed); fixture 10+10 guards biting first try; measurements show only intended edges. **E1** below. |
| S10 fixtures/guards | PASS | S9 guards green first try (check_fixture); structure/scip/diff/path/caller goldens classified per hunk (phase-9 record). |
| S11 repos | PASS | `var/m6b/bench/phase10.md` re-derived in this pass (counts match; gin/express/ftgo/mattermost transitions and microservices/piggymetrics byte-identity confirmed from the recorded dumps). Post-fix trigger grep (above): no E1/E2 shape in any measured repo, so the records stand. |
| S12 determinism/inv.1 | PASS (re-run in pass 2) | e2e determinism rows green at baseline; fixtures byte-identical post-S9 (phase-9 loop). E1/E2 touch only untriggered shapes on fixtures (re-verified by pass-2 e2e, no re-bless). |
| S13 growth | PASS | S4/S6/S8 linear tests green (quadratic mutants bite, logs verified). S9 adds no unbounded walk: every new loop is single-pass over facts (fetch_class, http_client, hooks, member resolution) — by design note, no new growth test required. |
| S14 loop | PASS (re-run in pass 2) | Phase-9 loop green; pass-2 repeats it on the fixed tree. |

## Findings (fixed test-first as amendments of phase 11)

- **E1 (S9): reconfigured fetch-class calls stitched plain.** Probes (new unit `evaluator_pass1_reconfigured_fetch_class_calls_are_capped`, failed first with 5 plain rows): a `superagent` plugin link (`.use`), `got`/`ky` interceptor/transport options (`hooks`, `agent`) and by-name/spread options, and `undici` `dispatcher` all stitched at carrier-plain — a synthetic-eligible shape through a configuration surface, contradicting D8. Fix (same phase): `configured_opts` (inline `hooks`/`agent`/`dispatcher` keys; spreads and names cap) on got/ky/undici direct+verb sites, and a `use`-link check capping superagent verb/direct sites. Leading `.use` stays a miss with its token hook (safe). 5 fix mutants killed (`p11-mutants.sh`: plugin/hooks/by-name/spread/dispatcher arms). Residue kept: `prefixUrl`-in-options unread (suffix at worst, never synthetic — scope Any), `undici` pools/clients, `got.paginate`/`stream` as non-sites.
- **E2 (S8): the load-balancer type itself lifted.** Probes (new positive rows, failed first): an imported or fully qualified `ReactorLoadBalancer` use did not resolve — a routing type through which requests re-route, so a lift could restore a wrong `synthetic`. Fix: `ReactorLoadBalancer` joins `DISCOVERY_TYPES` (comment restated). 1 fix mutant killed. Residue kept: other tree names (`StaticServerList` lives outside the discovery packages), generics-nested uses, bare uses with no resolving import.

## Confirmations (no finding)

- P4 axios instance-destructure base flow (`const {get} = api` → instance base, unverified): green first try.
- P5 hand-rolled recorder unmarked: green first try.
- P6/P7 same-package and non-binding-import lifts: green first try.
- P7 literal-`@` host: green first try.
- Phase-9's own two survived-mutants-turned-rows (module-text fallback, import-name gate) stand as designed.

## VERDICT: PASS (with fixes E1, E2 — both test-first with killed mutants, records in phase 11)
