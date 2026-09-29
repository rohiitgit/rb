# M1b contract: Tree-sitter syntax extraction

Written by the planner before any code. M1b is done only when every assertion below is backed by a passing test or a recorded measurement.

## Scope

The M1b pipeline:
- Parse TypeScript, TSX, JavaScript, Java, Python and Go blobs with Tree-sitter.
- Extract definitions and references per file into `FileFacts`.
- Cache `FileFacts` by `(blob SHA, extractor version)`.
- Turn definitions into `Symbol` nodes with `DEFINED_IN` and `CONTAINS` edges.

References are extracted and cached, but they do **not** become graph edges yet. Resolving them into `CALLS` is M3 (design §5.4), and emitting unresolved edges now would add state that M3 then has to undo.

Dropped from M1b: `gix` change sets. The blob-keyed facts cache already skips unchanged files. Change sets only matter for SCIP project invalidation in M3.

## Engineer declaration

**Canonical state:**
- a blob's `FileFacts`: definitions `(kind, name, name span, node span, parent index)` and references `(kind, name, span, enclosing definition index)`;
- the cache row `(blob, extractor version) → FileFacts`.

**Derived:**
- `Symbol` node key, from file path + ancestor chain + name + kind + ordinal among same-named siblings;
- `DEFINED_IN` edge (Symbol → File);
- `CONTAINS` edge (parent Symbol → child Symbol).

**Invariants:**
1. `FileFacts` is a pure function of `(blob bytes, language, extractor version)`.
2. Every `Symbol` has exactly one `DEFINED_IN` edge, and at most one incoming `CONTAINS` edge.
3. Symbol keys are unique per snapshot.
4. Warm-cache output = cold-cache output, byte for byte.

## Assertions

| # | Assertion | Proof |
|---|---|---|
| C1 | For each language there is a fixture file whose extracted definitions (kind, qualified name, 0-based name span) equal a hand-written expectation | unit tests in `rb-syntax`, one per language |
| C2 | A method inside a class has that class as its parent; the graph has `CONTAINS` class → method | unit + dump golden |
| C3 | A function nested in a function is a symbol whose parent is the outer function | unit (TS, Python) |
| C4 | Overloads and same-named siblings get distinct keys, numbered in source order: `login().`, `login(+1).` | unit (Java overloads) |
| C5 | A symbol's key depends only on its own file's contents and path, never on other files or on processing order | unit: extract a file alone and alongside others, same keys |
| C6 | `DEFINED_IN` edge evidence = the definition's name span in its file | unit + dump golden |
| C7 | Calls are extracted as references carrying their enclosing definition; no `CALLS` edges are emitted in M1b | unit + assertion that the dump has no CALLS |
| C8 | A file with syntax errors still yields the definitions from its well-formed parts, plus one `syntax_error` diagnostic at the first error; no panic | unit |
| C9 | Binary, generated and unsupported-language files are never parsed. Files over 1 MiB are not parsed and get an `extraction_skipped` diagnostic. | unit |
| C10 | Re-indexing a commit with a warm cache parses 0 files and produces a byte-identical dump | e2e: parse counter + dump equality |
| C11 | A different extractor version misses the cache and re-parses | unit on the cache key |
| C12 | Parallel extraction gives the same `FileFacts` as serial extraction | unit over the fixture set |
| C13 | Spans are 0-based lines with byte columns; CRLF files and non-ASCII identifiers give correct spans | unit |
| C14 | `.ts`/`.mts`/`.cts` use the TypeScript grammar, `.tsx` the TSX grammar, and `.js`/`.jsx`/`.mjs`/`.cjs` the JavaScript grammar | unit |
| C15 | AGE parity holds with `Symbol` vertices and the new edges | existing e2e parity test |
| C16 | `rb find login --kind symbol` returns the login symbols from the fixtures | golden |
| C17 | Symbol keys follow SCIP descriptor shape with an `rb-heuristic` scheme; names with non-identifier characters are backtick-escaped | unit |
| C18 | Real repos (a private Java repo and a private TypeScript repo) index with no panic, and their symbol counts are sanity-checked by hand against `grep` | recorded measurement: cold time, warm time, symbol count |
| C19 | No new dependency without a stated reason; only Tree-sitter and grammar crates are added (the cache uses the existing serde_json, no bincode/zstd yet) | review |
| C20 | `cargo fmt --check`, `clippy -D warnings` and the full test suite pass; the stub scan is clean | verification loop |
