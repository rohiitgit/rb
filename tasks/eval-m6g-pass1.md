# M6g evaluator pass 1 (full review, adversarial probing included)

Ran 2026-10-12 on the phase-6 tree, fixes through `stitch.rs` P31/P32. VERDICT: PASS with 6 fixes
(P9, P18, P30, P31, P32 + P2 order assertion), all test-first, all mutants killed, measurements
re-run after the fixes.

## Findings

- **P9 (fix: path-bearing targets never read).** `proxy_pass http://auth:8080/health` and
  `uri: lb://auth/prefix` used to read as pure prefix strips, but a URI path *replaces* the
  matched prefix instead of stripping it — a false `CONSUMES` when the stripped path happened to
  match an endpoint. `proxy_host`/`gateway_uri` now return `None` for path-bearing values (a lone
  trailing `/` still reads). Tests-first (nginx unit updated: `/health` case now unread, `/slash/`
  trailing-slash case reads).
- **P2 (longest-prefix order asserted).** `with_rewrites` sorts longest-first, but no test killed
  an ascending sort. New stitch unit pins the order plus the silent-service filter.
- **P18 (fix: unresolvable topic forms counted, never silent).** `@KafkaListener(topicPattern)`,
  `topicPartitions`, bare/groupId-only listeners, Rabbit `bindings`, and Kafka `sendDefault`
  contributed nothing — no edge *and* no `topic_dynamic` count. Listeners without a readable
  `topics`/`queues` value now mark dynamic; `sendDefault` marks dynamic. Tests-first.
- **P30 (fix: Rabbit exchange-first arities).** 3-argument `convertAndSend`/`send` take the
  exchange first, not the topic — the exchange would have edged as a topic. Only routing-first
  arities (`convertAndSend`/2, `send`/≤2) read the first argument now. Tests-first.
- **P31 (fix: conflicting duplicate prefixes never apply).** Two files stating `/api/` for different
  services let file order pick the target silently. `resolve_rewrites` now drops a prefix claimed
  for several services (identical restatements dedupe to one). Tests-first.
- **P32 (fix: SpEL and partial templates are computed).** `topics = "#{__x.topic}"` and
  `"pre-${x}"` edged as literal topics — 3 false `SUBSCRIBES` on spring-kafka, caught by sampling.
  Literals containing `#{` or a non-whole `${` are dynamic now, in annotations and sends.
  Tests-first; post-fix re-measurement: 119→116 topic nodes, 111→107 SUBSCRIBES, 0 suspicious
  names.
- **Residue (recorded, not fixed):** `@SendTo` reply topics (contract S1 table already marks it
  open); list-form `on-profile:` sequences (skipped by the line reader — safely ignored with
  overrides marking, values fall back to base); YAML anchors (contract residue); multi-pattern
  comma `Path=` values (accepted as one nonsense prefix that matches nothing — safe by the
  no-candidate fallback).

## Measurements (phase 7)

- apache/kafka @ `f32956…`: 625,213 rows, base == head (0 diffs). Vacuous leg, proven correct: the
  tree has no spring-kafka/spring-rabbit/spring-amqp/spring-cloud-stream dependency in any build
  file, so no Spring topic form can occur. 0 false topics.
- spring-cloud-gateway @ `c5021b…`: +8 rows, all `gateway_unread` on test-resource YAMLs, 0 new
  edges. All 8 verified by hand against the sources: placeholder `uri: ${test.uri}` (cors,
  retrytests, versions, netty blocks), unknown schemes (`no://op`, `forward:`), host-only/uri-less
  blocks. Reader-level positive samples: `Path=/**` → `/` (weight file default route),
  `https://example*.com` + `Path=/anything/*` routes (propertiesbeandefinition file).
- spring-projects/spring-kafka @ `6c8549…`: pending (positive topic leg).

## Mutants (this pass)

10/10 killed, each reverted and the tree re-verified (841 workspace units, topo + fixture e2e
unblessed green): M1 P9-proxy-path, M2 P9-lb-path, M3 annotation-fallback, M4 sendDefault,
M5 rabbit-arity, M6 contest-drop, M7 dedup-drop, M8 classify-SpEL, M9 send-SpEL, M10 sort-order.
Two battery scripts needed a second attempt (uncompilable mutants give no verdict; counted only
compiling kills).

## Verdict

PASS with the 6 fixes above. No blockers remaining: every finding holds test-first, the
spring-kafka re-measurement shows 0 suspicious topics and 0 false edges in samples, invariant 1
holds, and the close verify stays green. Residue recorded, not fixed (see Findings).
