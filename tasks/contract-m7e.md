# M7e contract: input hygiene (stress batch D) (2026-10-13)

**Status:** draft written 2026-10-13 from `tasks/stress-findings.md` batch D; not started. Stacked on M7d.

## Why

| Finding | What the stress campaign found |
|---|---|
| F16 | Go `vendor/` is indexed as part of the repo. go-gin-example: 1,020 of 1,073 files, 14 extra projects (one "deployable service"), about 10× graph inflation, `sql:` status entirely from vendored gorm. |
| F29 | Go build-tag twins (`isatty_bsd.go`/`isatty_linux.go`) give 1,059 ambiguous references. |
| F22 | Committed build output (`dist/`) duplicates every symbol (nestjs-realworld). |
| F13 | There is no way to keep files out of parsing: `rb.toml` `exclude` only drops manifests, so a committed `node_modules/` parses 3,000 files of noise. |

## Claims

| # | Claim | Mechanism | Residue |
|---|---|---|---|
| S1 | **One exclusion decision per file, made once.** A file is *excluded* when an `rb.toml` `ignore` glob matches, or by a default rule (S2–S4). An excluded file keeps its `File` node (ownership, `rb find`), but is not parsed, raw-scanned, used for project detection or included in SCIP jobs. Every exclusion is counted as one `excluded {reason, files}` diagnostic per reason, and `rb status` prints it. | `rb-git` manifest `FileEntry.excluded: Option<&'static str>`; consumers in `rb-git` boundaries and `rb-indexer` syntax/projects, `rb-scip` jobs | `rb.toml` `include` overrides for the defaults (an explicit opt-back-in) |
| S2 (F16, F29) | **Go `vendor/` is excluded by default** when a `go.mod` sits beside a `vendor/modules.txt`. | default rule | Build-tag selection (F29) is mostly resolved by S2; any remainder is recorded |
| S3 (F13) | **Committed `node_modules/` is excluded**, as is anything `.gitattributes` marks `linguist-vendored` or `linguist-generated`. | default rule | `.gitattributes` macros and negations unread |
| S4 (F22) | **TypeScript build output is excluded**: a `tsconfig.json` `compilerOptions.outDir` directory, relative to that tsconfig. | default rule from manifests | `outDir` set in an `extends` chain unread |

## Verification

- Tests first per rule, with mutants.
- Real-Docker suites: 0 failures and 0 skips.
- Invariant against M7d. Fixtures are expected identical, except any that commit vendor or build trees. go-gin-example, nestjs-realworld and mattermost (if it vendors) are expected to shrink. Every removed row is classified as a vendored or build file.
- `PIPELINE` bumped to `m7e-1`.
