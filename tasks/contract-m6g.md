# M6g contract: topology and configuration packs (draft, 2026-10-12)

**Status:** contract written 2026-10-12 on branch `rohiitgit/m6b-phase-3-status-v1` (M6e/M6f merged
to `origin/main` as PR #1); forks F1–F6 decided below; phase 0 not started.

Written before any code. M6g is done only when every assertion below is backed by a passing test or a
recorded measurement. "Verified facts" are what the 2026-10-12 probes read in the tree at merge
`f80714d` (M6f closed; `PIPELINE` `m6f-1`).

This contract follows the M6e/M6f format and applies the lessons M3b–M6f paid for (soundness bar +
closing bar up front; allow-lists; suppressions counted; growth by counted ops; no hand-counted
positions; invariant 1 vs previous close with classified rows; fixture plan is code; config-surface
probing; no builds during container e2e; non-empty dumps).

## Soundness claims and closing bar (read first)

M6g's outputs whose failure would be silent: a topic edge on the wrong topic (a misread literal,
a dynamic topic treated as static); a gateway rewrite that misroutes (wrong prefix strip, wrong
service); a host mapping that merges two services (an over-broad compose name); a Spring profile
evaluation that applies the wrong document (wrong active profile, wrong document order); a YAML
misread that moves a value. **Everything not proven takes the safe direction:** an unreadable
topic/gateway/compose/YAML file contributes nothing (counted); a dynamic topic is never an edge;
an ambiguous rewrite is never applied (the direct form stays); an unknown profile key keeps
today's `.properties` reading; duplicate topic declarations never merge silently.

| Surface | Claim (exactly this, no more) | Mechanism | Residue (out of scope by construction) |
|---|---|---|---|
| Topic packs (S1) | Kafka (`@KafkaListener`, `KafkaTemplate.send`, `@SendTo`?) + RabbitMQ (`@RabbitListener`, `RabbitTemplate.convertAndSend`) topic literals read exactly as written | `rb-frameworks` packs over decorations/call chains; literal topics only | Dynamic topics (variables, patterns, SpEL), Kafka Streams topologies, headers/keys/partitions |
| Topic graph (S2) | `Topic` nodes keyed by literal; `Service -PUBLISHES-> Topic` and `Service -SUBSCRIBES-> Topic` (both originate at services); new `EdgeKind`s with store/AGE parity | `rb-model` kinds + `rb-store` projection + parity tests; plain edges | Consumer groups (recorded, not keyed); exactly-once semantics unmodelled |
| Gateway packs (S3) | nginx `location`+`proxy_pass` and Spring Cloud Gateway `Path`+`uri` routes become stitch rewrites (prefix strip + service map) | Line readers (nginx) + hand-rolled YAML walk (gateway); applied in stitch scope resolution | Rewrites (`rewrite` directive), regex locations, weight/load-balance, filters beyond strip-prefix |
| Host names (S4) | Compose service names, k8s `Service` names and `rb.toml` host mappings extend the D6 host rule | Readers for `compose.yaml`/`docker-compose*.yaml|yml`, k8s `Service` manifests, `rb.toml` `hosts` (schema extension) | Host networking modes, external links, `extra_hosts` |
| Spring YAML (S5) | `application.yml`/`.yaml` (incl. multi-document `---` + `spring.config.activate.on-profile`) and `bootstrap*` read; active profile evaluated from the documented precedence | Hand-rolled line reader (no new dependency); documents selected by active profile only | Random values (`${random.*}`), encrypted properties, config-server imports |
| Fixtures + guards (S6) | `topo-packs` fixture (Kafka producer/consumer, nginx + gateway YAML, compose + k8s names, YAML profiles) + polyglot gateway goldens; per-class biting guards | Fixture plan is code; e2e fns + goldens | — |
| Public repos (S7) | Classified diffs + read-in-source samples; 0 false topics/rewrites/hosts | Measurement plan below | New forms recorded for their owners |

**What M6g must not change.** DI/RTA/orphan/`INJECTS` rules untouched. `CONSUMES` never `exact` (reaffirmed).
`PIPELINE` becomes `m6g-1` at the first dump change, bumped on each later one.

This is invariant 1, checked against `f80714d` dumps of every fixture (with and without SCIP) and of the M6g
measurement repositories at their pins, every differing row classified by script (dumps asserted non-empty first).

**Closing bar.** Decided now:
1. **Evaluator pass 1 is a full review,** adversarial probing included (topic confusion, rewrite smuggling, host merging, profile games, YAML smuggling).
2. **Evaluator pass 2 is verification-only.**
3. **Blocks closing only:** (a) a pass-1 fix that does not hold; (b) a regression; (c) a false topic/rewrite/host edge above its stated certainty through a controlled form; (d) non-determinism; (e) a failed test/guard/step; (f) an AGE parity mismatch on any fixture.
4. **Not blocking:** a new form in a residue class (recorded); a precision miss contradicting no rule; §12.5 timing (measured, not asserted).
5. **Timings are measured, not asserted.**

## Scope

**In (M6g):** the 7 items above (design §13 M6g row; contract-m4a items 3/10/13/18; contract-m4b active profiles).

**Out of M6g (each assigned):** `rb check` → M6h; Kotlin/C# → M6i; Guice/Dagger/NestJS/Spring-XML/.NET → M6j; dynamic topics, streams topologies, rewrite directives, YAML anchors, config-server → residue.

## Engineer declaration

**Canonical state:** new readers (topics, nginx, gateway YAML, compose/k8s, Spring YAML/profiles); `Topic` nodes + `PUBLISHES`/`SUBSCRIBES` edges; gateway rewrites + host names in stitch scope; profile-evaluated config; `rb.toml` `hosts`; `PIPELINE` `m6g-1`… at each dump change. New counters only when non-zero.
**Derived:** topic edges, rewritten stitches, host-matched sites, effective config.
**Invariants:**
1. **Dumps.** `--no-scip` and SCIP dumps differ from `f80714d` only in classified rows; nothing unclassified; every compared dump asserted non-empty.
2. **Kinds.** Two new edge kinds (`PUBLISHES`, `SUBSCRIBES`) + one node kind (`Topic`), each with a producer, a fixture and AGE parity (§12.2); nothing else new.
3. **Certainty monotonicity.** No topic edge above `heuristic` (literals are stated but routing is broker-mediated); no `exact` version from anywhere new.
4. **Egress closure.** Unchanged (M6g adds no fetches).
5. **Determinism.** Fresh == cached, two names, forced rebuilds: byte-identical on fixtures and measurement repos.
6. **Bounded work.** Every new walk has a visited set + caps; growth ≤ 5× ops at 4× input per new walk.
7. **Stated complexity by counted operations** for each new walk.

## Decisions (made here, with reasons)

- **D1: topic edges originate at services (F1).** `Service -PUBLISHES-> Topic` and `Service -SUBSCRIBES-> Topic`: a consumer subscribes, so the edge reads naturally from the service side, as does publishing. Both originate at services (uniform with `DEPENDS_ON`/`QUERIES`); Q3-style roll-ups walk both directions. Topic key `topic/<name>` (decided in phase 2 with the key-shape test).
- **D2: literals only (F2).** A topic from a string literal (annotation value, first arg) is an edge; anything computed is `topic_dynamic` + counted. Placeholders (`${…}`) resolve through Spring properties when unique, else dynamic.
- **D3: gateway rules rewrite, not nodes (F3).** An nginx `location P { proxy_pass http://S...}` or gateway `Path=P, uri=...S` means "calls to P... reach service S": applied as a stitch rewrite (strip P, scope to S), reusing the context-paths precedent. No new node kind for gateways.
- **D4: hosts extend D6 (F4).** Compose `services:` names, k8s `Service` `metadata.name`s and `rb.toml` `[[hosts]]` (new schema: service dir + names) join the host→services map the stitcher already reads. Duplicates across services stay ambiguous (never merged).
- **D5: YAML hand-rolled, profiles evaluated in documented precedence (F5).** `application.yml` multi-document with `spring.config.activate.on-profile`; active profile from (lowest first) `application.properties`, `application.yml` (`spring.profiles.active`), `.mvn`... no — Spring's order: `spring.profiles.active` in properties/YAML, then `SPRING_PROFILES_ACTIVE`? (env is host-dependent — NEVER read env; recorded). `bootstrap.*` read as lowest precedence. Unreadable YAML keeps today's `.properties` reading.
- **D6: fixture plan is code:** `fixtures/topo-packs/` (Kafka producer + consumer with literal + dynamic topics, nginx.conf, gateway `application-gateway.yaml`, `compose.yaml` + k8s `svc.yaml`, `application.yml` with two profiles + `bootstrap.properties`); polyglot gateway goldens extended (`gateway/nginx.conf` already there: `location /api/` → auth).
- **D7: measurement reuses pins where they exist;** new pins (a Kafka-carrying repo, a gateway-carrying repo) chosen in preflight by `git ls-remote` + API, never by cloning first.

## Assertions

| # | Assertion | Proof |
|---|---|---|
| S1 | **Topic packs.** Kafka/Rabbit listener + send literals; dynamics out. | unit; mutation |
| S2 | **Topic graph.** New kinds with producer + fixture + AGE parity. | unit; mutation; e2e goldens |
| S3 | **Gateway packs.** nginx + Spring Gateway rewrites; regex/dynamics out. | unit; mutation; e2e goldens |
| S4 | **Host names.** compose/k8s/rb.toml extend D6; duplicates ambiguous. | unit; mutation; e2e goldens |
| S5 | **Spring YAML.** Multi-doc profiles evaluated; unreadable keeps today. | unit; mutation; e2e goldens |
| S6 | **Fixture + guards.** Per-class must-exist/must-not-exist, each biting. | e2e with containers; goldens |
| S7 | **Public repos.** Classified diffs + samples; 0 false topics/rewrites/hosts. | recorded measurement |
| S8 | **Determinism + invariant 1.** Fresh == cached, two names, forced rebuilds; dumps vs `f80714d` classified. | e2e; recorded |
| S9 | **Growth by counted operations** (invariants 5–6). | unit |
| S10 | **Verification loop + closing.** `var/m6g/scripts/verify.sh`: fmt, clippy, counted units, store/server with DB, e2e + api with containers, listener + stub checks. | verification loop |

## Verified facts (read at `f80714d`, 2026-10-12 probes)

1. No topic/gateway/compose/k8s/Spring-YAML fixtures or code paths (only `gateway/nginx.conf` content in polyglot: `location /api/ → auth:8080`; java-spring-jpa's `KafkaAuditSink` is an unwired stub comment).
2. `EdgeKind` is a closed `string_enum` (no `PUBLISHES`/`SUBSCRIBES`); the string form is both `rb.edge.kind` and the AGE edge label.
3. `rb.toml` is `deny_unknown_fields` with only `exclude`/`service` — no host mappings.
4. The stitcher reads hosts from a services map (D6); Spring config reads `application.properties` only (YAML/YML/profile/bootstrap unread).
5. No YAML parser in the workspace (M6f D1 precedent: hand-rolled line readers).
6. `Topic` node kind does not exist; design §6.5 reserves the shape only in prose.

**Not verified (checked in preflight, before the step that needs it):** Kafka/Rabbit annotation shapes in the wild (listener/send arg positions); nginx directive shapes beyond location/proxy_pass; gateway YAML route shapes; compose/k8s field shapes; Spring profile-precedence docs; measurement pins.

## Fixture plan

- `fixtures/topo-packs/`: Java Kafka producer (`KafkaTemplate.send("events", …)` + a dynamic-topic decoy) + consumer (`@KafkaListener(topics = "events")`), `nginx.conf` (2 locations), Spring Gateway YAML (2 routes), `compose.yaml` (2 named services), `k8s.yaml` (1 Service), `application.yml` (2 profile documents + base) + `application.properties` + `bootstrap.properties`. Goldens: topics, gateway paths, host-matched stitches, profile-effective config, status lines.
- Polyglot: gateway goldens extended over the existing `gateway/nginx.conf`.

## Public-repository measurement plan

| Step | Repository @ pin | Runs | Notes |
|---|---|---|---|
| Phase 0 | — | `f80714d` binary dumps (`--no-scip`); e2e green baseline | small |
| Phase 7 | Kafka-carrying repo @ pin (chosen preflight) | `--no-scip` both binaries | classified diffs, samples |
| Phase 7 | gateway/compose-carrying repo @ pin | `--no-scip` both binaries | classified diffs, samples |
| Phase 7 | existing public repos | N/A (no new reads on old shapes) | — |

Disk policy as M6b D9 (6 GiB floor, one repo at a time, remove + `VACUUM`, part-by-part figures).

## Determinism

No wall-clock/host-path/run-order in dumps (unchanged rules). Fresh == cached, two names, forced rebuilds on fixtures (e2e); `--no-scip` both binaries byte-identical modulo classified rows.

## Verification loop

`var/m6g/scripts/verify.sh <tag>` (copied from M6f's, M6g paths/logs): fmt, clippy `-D warnings`, counted units, `rb-store`/`rb-server` with DB, e2e + api with containers, listener + stub checks, mutation runs per M4a harness rules.

## Forks (decided here, with reasons)

- **F1: topic edges originate at services.** Uniform with manifest/query edges; Q3 walks both directions.
- **F2: literals only.** Dynamics counted, never edges.
- **F3: gateway rules rewrite.** No new node kind; context-paths precedent.
- **F4: hosts extend D6.** Duplicates stay ambiguous.
- **F5: YAML hand-rolled, profiles evaluated.** Env never read (host-dependent); unreadable keeps today.

## Amendments

1. **(phase 0) Baselines, pins and shape checks.** Baseline binary `var/m6g/base/rb-f80714d` (release, sha256 recorded; HEAD code == `f80714d` — the contract commit adds no code). Full-SHA pins verified by `git ls-remote`: apache/kafka `f3295655931e6d2b255f20886ccb697a93a6bbe9` (HEAD; canonical topic shapes), spring-cloud/spring-cloud-gateway `c5021b656f3ae7c45adc842db106ed90c778bcc6` (HEAD; canonical gateway+YAML shapes). Sizes accepted per M5 precedent (parse-only `--no-scip` handled 1.7M lines; one repo at a time, remove + `VACUUM`). Shape checks: no topic/gateway/compose/k8s/Spring-YAML fixtures or code (only polyglot `gateway/nginx.conf` content and the spring-jpa unwired-Kafka stub comment); `EdgeKind` closed without topic kinds; `rb.toml` deny-unknown-fields without hosts; D6 host rule and `.properties`-only config as documented; no YAML parser. Forks F1–F5 decided in the contract with reasons. Baseline verify `var/m6g/logs/p0-baseline-*` green (fmt, clippy, 842 units, store 66, server 12, e2e 37/37, api 1/1 over 150 sets, no `skipping:`, no listener, stub scan clean). DB holds only the 3 pre-existing `fx_*` repos.

2. **(phase 1) Topic packs.** `rb-frameworks/src/topics.rs`: `topics()` for Java (other languages return empty, recorded residue) — listener annotations (`@KafkaListener(topics)`, `@RabbitListener(queues)`: string/array/placeholder/dynamic) and template sends (`*KafkaTemplate.send`, `*RabbitTemplate.convertAndSend|send`: written receiver or binding-declared type per the rest_template precedent; first-arg literal/placeholder, computed args dynamic). Placeholders kept for the indexer; dynamics counted with spans, never edges. Tests (3 units + growth ratio 4x→5x via `work::tick`); mutants killed: listener key, template-suffix widening (grew a declared-`MailTemplate` decoy after the first widening survived — undeclared roots never reach the check), plus the pack-level shapes. No graph change. Verify: fmt, clippy clean; 169 frameworks units.

3. **(phase 2) Topic graph.** `rb-model`: `NodeKind::Topic` (`topic`/`Topic`) + `EdgeKind::{Publishes, Subscribes}` (existing `ALL` roundtrip covers them; workspace clippy clean — no exhaustive match broke, and the AGE projection iterates `ALL` kinds generically). `rb-indexer/src/topics.rs`: `add_topics` wired into assemble after the SQL stage — service-originated edges (`Heuristic`, `Heuristic` provenance), `topic/<name>` nodes, placeholder resolution through the owning service's Spring properties (single lookup; nested/missing → `topic_dynamic`), computed sites → `topic_dynamic`, empty literals skipped. Tests first (2 stage tests failed on missing variants); mutants killed: kind swap, placeholder-never-resolves, certainty-raised. AGE parity flows through the generic projection (proven on the fixture in phase 6). No fixture carries topics: no dump change. Verify: fmt, clippy clean; 308 indexer + 19 model + 169 frameworks units.

4. **(phase 3) Gateway packs.** `rb-frameworks/src/gateway.rs`: `is_gateway_input` (`nginx.conf`, Spring `application*`/`bootstrap*` YAML) + `nginx_gateway` (literal `location` + exactly one `proxy_pass http(s)://host`, no `rewrite`; `~` regex, `$` variables, malformed counted unread) + `spring_gateway` (`routes:` items with `uri:` http(s)/`lb://` and `predicates: Path=` prefixes; tabs fail the file; unshaped blocks counted). Tests first (reader tests passed pre-wiring; stitch tests failed pre-fix on resolution dedup and bucket expectations); mutants killed: regex-read-as-rule, rewrite-ignored, variable-target-read, `lb://`-dropped, single-`*`-accepted, scope-not-narrowed, attr-removed, ambiguous-resolved. `rb-indexer`: `Content.gateways` collected by name; assemble parses per filename, resolves `(prefix, target)` through `Stitcher::resolve_rewrites` (exactly one service, else dropped), stitches via `add_consumes_with` (empty rewrites keep the old path, so old callers are untouched); `Stitcher::with_rewrites` keeps only targets exposing endpoints, longest-prefix-first; `site()` applies a rewrite only when the host does not already pin to an exposing service, only when the stripped path yields a candidate (else the direct form stays), marking `via_gateway`. `gateway_unread` diagnostics ride only when non-zero. Polyglot's `gateway/nginx.conf` (`/api/` → auth) strips to `/v1/login`, which matches no endpoint, so the direct edge stays: no dump change expected (proven in the phase-6 verify). Verify: fmt, clippy clean; 311 indexer + 172 frameworks units.

5. **(phase 4) Host names.** `rb-frameworks/src/hosts.rs`: `is_compose_input` (`compose.yaml`/`.yml`, `docker-compose*.yaml`/`.yml`) + `is_hosts_input` (compose or any YAML) + `compose_names` (top-level `services:` keys one indent deeper; tabs fail) + `k8s_service_names` (`metadata.name` of `---`-separated `kind: Service` docs only; Deployments/dynamics unread) + `rb_toml_hosts` (hand-rolled `[[hosts]]` `service =`/`names = [...]` reader, no new dependency; malformed reads as nothing). Tests first (4 reader units); mutants killed: nested-key read, Deployment read, dynamic accepted, malformed accepted. `rb-git`: `RbToml.hosts: Vec<toml::Value>` so `[[hosts]]` parses (detection ignores it) + unit. `rb-indexer`: `Content.host_files`/`rb_hosts`; assemble owns names by path, root-matches by key basename/name, counts `hosts_unmatched` (rb.toml dirs validated against project dirs); `Stitcher::add_host_aliases` joins the host map (unknown keys dropped); `add_consumes_with` carries aliases (empty keeps the old path). Stitch tests first (failed pre-fix on Synthetic-vs-Heuristic and ambiguous-resolution expectations); mutants killed: alias-not-pinned, duplicates-merged, unknown-applied. No fixture carries compose/k8s/hosts: no dump change expected (proven in the phase-6 verify). Verify: fmt, clippy clean; 312 indexer + 176 frameworks + 27 git units.

6. **(phase 5) Spring YAML.** `spring_config` evaluates profiles instead of only counting them: `select_yaml_documents` splits `application.yml`/`.yaml` into base (unconditional) and selected-profile (`on-profile`/legacy `spring.profiles` naming an active profile) documents; tabbed YAML contributes nothing. `active_profiles` reads `spring.profiles.active` from unconditional documents only (a gated document never self-activates; profile files never select). Merge precedence, lowest first: bootstrap, YAML base, `.properties`, YAML profile docs, profile YAML, profile properties (profile files beat base files; `.properties` beats `.yml` per Spring's file order; bootstrap lowest). Diagnostics, overrides and `unread` stay exactly as before (profiles can also arrive via env/CLI, which are never read, so the caps stay conservative); `spring_config_unread` now fires only for files that contributed nothing. `context_paths`, folding and topic placeholders flow from the merged values. Tests first (2 new value-selection tests green; the 3 existing config tests pass unchanged); mutants killed: staging-doc-applied, inactive-file-merged, bootstrap-winning, yaml-beating-props, tabbed-yaml-read, self-activation. No fixture carries YAML/profiles: no dump change expected (proven in the phase-6 verify). Verify: fmt, clippy clean; 314 indexer + 176 frameworks + 27 git units.

7. **(phase 6) Fixture + guards.** `fixtures/topo-packs/`: Kafka producer (`send("${app.topic}")` resolved through the active profile's YAML document to `prod-events`, plus a computed decoy) + consumer (`@KafkaListener(topics = "prod-events")`), Spring `POST /v1/login`, TS client with one function per call (nginx `/api/`+`/pay/`, gateway `/bill/`, `rb.toml` alias `login`, k8s alias `auth-http`), `nginx.conf`, gateway YAML (2 routes + 1 unread `/w/*`), root `compose.yaml` + `k8s.yaml`, `rb.toml` (`[[service]]` gateway + `[[hosts]]`), profile `application.yml` files + `bootstrap.properties`. e2e `topo_packs`: determinism (two repos + force), store round trip, guards + HTTP/M4b invariants, file set, AGE parity (whole-graph plus `PUBLISHES`/`SUBSCRIBES` label checks, proving the generic projection), `dump.jsonl` golden (reviewed by hand, 102 lines), endpoint neighbours, read-only cypher. 11 must-exist + 4 must-not-exist guards, each biting across 8 mutant runs (precedence both directions, consumer literal, rewrite-to-silent, unruled prefix, exact-certainty code mutant, method change, bootstrap value). `PIPELINE` `m6g-1` (no golden pins the string). Verify: fmt, clippy clean; topo e2e green unblessed.

8. **(phase 6 close) Determinism + invariant 1.** Close verify `var/m6g/logs/p6-full-*` green: fmt, clippy `-D warnings`, 861 workspace units (0 failed), store 66/66, server 12/12, e2e 38/38 with containers, api 1/1 (150 parameter sets identical), no `skipping:`, no listener, stub scan clean (only the known `rb-sql` match). `git diff 781d32a f80714d` empty (the merge added nothing), and the direct base-vs-head spot check (`var/m6g/scripts/p6-invariant.sh`, release binaries, dumps asserted non-empty) is IDENTICAL everywhere: all 10 old fixtures `--no-scip` (0 added/removed rows each) plus SCIP mode for `polyglot-monorepo` (396 rows) and `ts-express-prisma` (71 rows). SCIP scope decision: `--no-scip` compared on all fixtures; SCIP compared where no dump golden pins the mode (the other SCIP fixtures' dump goldens passed e2e unchanged, which proves their equality). No fixture carries M6g inputs except `topo-packs` (new) and polyglot's `gateway/nginx.conf` (rewrite falls back: stripped `/v1/login` matches no endpoint, direct edge stays).


9. **(phase 7) Public-repository measurements.** `--no-scip` base-vs-head dumps (release
binaries, dumps asserted non-empty, one repo at a time + `VACUUM`, trees removed after):
apache/kafka @ `f32956…` (625,213 rows, IDENTICAL — vacuous leg, proven correct: no
spring-kafka/spring-rabbit/spring-amqp/spring-cloud-stream dependency in any build file, so no
Spring topic form can occur); spring-cloud-gateway @ `c5021b…` (+8 rows, all `gateway_unread` on
test-resource YAMLs, 0 removed, 0 new edges — all 8 verified by hand: placeholder
`uri: ${test.uri}`, unknown schemes `no://op`/`forward:`, host-only/uri-less blocks; positive
reader samples: `Path=/**` → `/`, `https://example*.com` + `Path=/anything/*` routes);
spring-projects/spring-kafka @ `6c8549…` (+808 rows, 0 removed: 116 `Topic` nodes, 60 `PUBLISHES`,
107 `SUBSCRIBES`, ~525 `topic_dynamic`). Post-fix re-measurement (P9/P18/P30/P32): gateway
unchanged (+8 same rows), spring-kafka 111→107 SUBSCRIBES and 119→116 nodes (the 3 SpEL false
edges gone, now dynamics), 0 suspicious topic names remaining. Samples read in source (all
true): `template.send("annotated13", null, "")` → PUB; `@KafkaListener(topics =
"DltStartupTests.2")` → SUB; sample-01 `send("topic1")`/`topics="topic1"`/`topics="topic1-dlt"`
(PUB/SUB/SUB); `template.sendDefault(0, 0, "foo")` with variable default topic → dynamic (the
P18 case in the wild). 0 false topics/rewrites/hosts in all samples. New pins verified by
`git ls-remote` (full SHAs above).

10. **(evaluator pass 1) VERDICT PASS with 6 fixes.** Adversarial battery P1–P33
(`tasks/eval-m6g-pass1.md`): P9 path-bearing proxy/uri targets never read (a URI path replaces
the prefix); P18 pattern/partitions/bindings listeners and `sendDefault` counted dynamic, never
silent; P30 Rabbit exchange-first arities gated (2-arg `convertAndSend`, ≤2-arg `send`); P31
contested prefixes (one prefix, several services) dropped, identical restatements deduped; P32
SpEL/partial-template literals dynamic (3 false `SUBSCRIBES` caught on spring-kafka by sampling);
P2 longest-prefix order asserted. All test-first; 10/10 mutants killed and reverted (two battery
scripts needed a rerun: uncompilable mutants give no verdict). Measurements re-run post-fix:
gateway unchanged, spring-kafka 111→107 SUBSCRIBES with 0 suspicious topics. Residue recorded:
`@SendTo`, list-form `on-profile`, YAML anchors, comma `Path=` values.

11. **(evaluator pass 2 + M6g close) VERDICT PASS.** Verification-only: every pass-1 repro
re-run green on the final tree (P9/P18/P30/P31/P32/P2 units; spring-kafka 0 suspicious topics);
10/10 pass-1 mutants cited killed (the only later change is a no-op clippy refactor plus tests,
touching none of the mutated lines); close verify `var/m6g/logs/p6-close2-*` green — fmt, 866
workspace units, store 66, server 12, e2e 38/38 with containers, api 1/1 (150 sets identical),
no `skipping:`, no listener, stub scan clean (the loop's clippy step ran pre-fix and failed on
`redundant_locals`; the fix is a committed no-op, re-proven `clippy -D warnings` clean on the
final tree); invariant-1 spot check re-run on the final tree IDENTICAL; topo e2e unblessed green.
**M6g closed.** `PIPELINE` `m6g-1`. Open items carried forward: `@SendTo` reply topics,
list-form `on-profile`, YAML anchors, comma `Path=` values (all recorded residue); M6h–M6j as
planned (M6h contract drafted at `tasks/contract-m6h.md`).
