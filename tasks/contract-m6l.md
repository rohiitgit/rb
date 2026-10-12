# M6l contract: XML diagnostic siting (2026-10-12)

**Status:** draft written 2026-10-12 at the M6k close (`f0c9fe7`).

Backlog from M6k pass 1 (P9 residue): `resolve_xml_points` reports
`di_unresolved` at the owner's path with XML-file spans — incoherent
coordinates no consumer can use. M6k fixed the same shape in its own helper;
the XML lane was left as precedent to avoid churning M6j goldens. This
milestone fixes it deliberately, with the churn reviewed.

## Why (read first)

A diagnostic is a pointer: (path, span) must locate the writing. Today an
unresolved XML ref points at the consumer class file with a span from the XML
document — every such diagnostic misleads. One-word fix, one golden re-bless.

## Soundness claims and closing bar (read first)

Only `di_unresolved` diagnostics from `resolve_xml_points` move (owner file →
XML file); spans already are XML coordinates, so nothing else shifts. Edges,
buckets, points, and all other diagnostics are byte-identical. **Safe
direction unchanged:** misses stay misses, only their siting becomes coherent.

| Surface | Claim (exactly this, no more) | Mechanism | Residue |
|---|---|---|---|
| Siting (S1) | Unresolved XML points site the XML document | `p.owner_path` → `p.xml_path` in the one call | `by_owner`/`unique` keep owner-keying (XML precedent, consumer-shaped) |

**What M6l must not change.** Everything except the diagnostic path of
unresolved XML points (spring-xml golden re-blessed with reviewed diff;
everything else byte-identical). `PIPELINE` becomes `m6l-1` at the golden
change (dumps change).

Invariant 1 vs the M6k close: all fixtures byte-identical modulo the
re-blessed spring-xml rows.

**Closing bar.** Decided now:
1. **Evaluator pass 1 is a short review** (one fix surface; adversarial probe:
   an unresolved XML ref in the fixture proves the path).
2. **Evaluator pass 2 is verification-only.**
3. **Blocks closing only:** (a) a non-diagnostic row shifts; (b) a lost edge;
   (c) a failed test/guard/step; (d) non-determinism.
4. **Not blocking:** §timing (measured, not asserted).
5. **Timings are measured, not asserted.**

## Scope

**In:** the one-word fix + unit + spring-xml re-bless + guard note + verify loop.
**Out:** `by_owner`/`unique` re-keying (consumer-shaped, separate milestone if
ever); anything else.

## Engineer declaration

**Canonical state:** XML `di_unresolved` sited in the XML document.
**Derived:** unchanged edges/buckets.
**Invariants:**
1. **Dumps** differ from M6k close only in spring-xml diagnostic paths.
2. **Old packs frozen.**
3. **Determinism** (e2e double-index covers).
4. **Bounded work** (no new walk).

## Decisions

- **D1: diagnostics only.** Edges/points keep owner-keying (stitch consumes
  `unique` by (owner-path, span); re-keying is a different contract).
- **D2: fixture proves it.** spring-xml needs an unresolved XML ref — check
  whether one exists; if not, add a minimal one (a ref to an unknown bean id).

## Assertions

| # | Assertion | Proof |
|---|---|---|
| S1 | **Siting.** Unresolved XML refs site the XML file. | unit; e2e golden |
| S2 | **Nothing else shifts.** All other rows byte-identical. | reviewed re-bless diff |
| S3 | **Determinism + invariant 1.** | e2e; recorded |
| S4 | **Verification loop + closing.** `var/m6l/scripts/verify.sh` standard. | verification loop |

## Verified facts (to be read at the M6k close commit, in phase 0)

1. Baseline HEAD `f0c9fe7`; baseline binary recorded.
2. `resolve_xml_points` diagnostic call site (`di.rs`, owner_path arm).
3. spring-xml fixture + golden paths.

## Fixture plan

- `fixtures/spring-xml/`: add an unknown-bean ref only if none exists.
- No changes to other fixtures.

## Verification loop

`var/m6l/scripts/verify.sh <tag>` (copied from M6k's, M6l paths/logs).

## Forks (decided here, with reasons)

- **F1: diagnostics only** (D1).

## Amendments

1. **(phase 0) Baselines.** Baseline binary `var/m6l/base/rb-m6kclose` (release,
sha256 `41e7d632…`, copied from the M6k final-tree build: no code changed
between its source and HEAD at copy time — only the contract draft commit).
Baseline is the M6k close by construction; no separate baseline verify (the
M6k `p6-close` loop is the standing green baseline; M6l re-runs it at close).

2. **(phase 1) Siting fix.** One-word change (`owner_path` → `xml_path`) +
unit (`xml_unresolved_diagnostic_sites_the_document`, mutant kills) +
spring-xml fixture (`missing` ref on the audit bean — no new nodes/edges, no
guard churn) + reviewed re-bless (coverage +1 point/+1 unresolved, new
diagnostic sited in the document, blob/hash only). Commit `ca0c641` (incl. a
clippy `needless_borrow` fix in the new test).
