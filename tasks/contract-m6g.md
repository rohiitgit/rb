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
