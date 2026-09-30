# Lessons

## 2026-09-29: numbers in reports come from the tool output, summed by a tool
- **Mistake:** I reported "188 unit tests" after a run that printed per-crate counts adding up to 153.
- **Rule:** Never state an aggregate I haven't computed from the tool output. Sum it with `awk`, or quote the per-crate lines as printed.

## 2026-09-29: "same algorithm" claims need an edge-case comparison, not a skim
- **Mistake:** I claimed TypeORM's `snakeCase` and Flask-SQLAlchemy's `camel_to_snake_case` were "the same two regex rules". They differ on names with underscores and leading capitals: `My_Table` gives `my_table` in TypeORM and `my__table` in Flask. I had also written that `snake_case` matched Hibernate, which it doesn't.
- **Rule:** Before calling two naming rules equivalent, run both over one table of edge cases: acronyms, digits, underscores, leading and trailing capitals, quoted names. Keep that table as a unit test.

## 2026-09-29: facts without imports can't identify a library by a bare name
- **Mistake:** I accepted a bare `Model` and any `*.models.Model` as Django, which caught Keras models.
- **Rule:** If a framework is detected by a short name, either use import facts or accept only the qualified form, and record the gap in the contract.

## 2026-09-30: a termination argument in a comment is a claim, so test it
- **Mistake:** I wrote "this terminates" above a round-based fixpoint whose per-name result could flip between rounds. Legal Java made it loop forever.
- **Rule:** Prefer structures that terminate by construction (memoised recursion with an on-path guard, visited sets) over fixpoints argued in comments. When a fixpoint is needed, add a test with a cycle and a test with a long chain.

## 2026-09-30: complexity claims need a timing test on a pathological input
- **Mistake:** I called the visited-set walks "linear". Each walk was linear, but it re-scanned the file for every base lookup and ran once per definition, which made the whole thing cubic. The same pass also showed that caching a result computed while a cycle was still open makes results depend on names.
- **Rule:** When a doc or comment claims a complexity, add a test that times a deep and a wide pathological input. Only cache results that don't depend on unfinished recursion.

## 2026-09-30: every recursion edge needs its cycle test, including indirect ones
- **Mistake:** I guarded the member and supertype walks, but not type resolution → qualifier → binding → type resolution. Real Python (`session = session.Session()`) overflowed the stack on psf/requests.
- **Rule:** List every call edge of a recursive component (A→B→C→A), not just the obvious self-recursion. Guard the component at one entry point, and give each edge a cycle test with a self-referential input.

## 2026-09-30: never hand-count positions in test expectations
- **Mistake:** the visibility-extent test had four expectations with columns I counted by eye. Each was off by one to three, and it took six test runs to converge, each time "fixing" the expectation to the output.
- **Rule:** Derive expected positions from the source in the test itself (`src.find("token")` plus a length), or assert relations ("`c`'s extent ends before `c.Next()`") rather than raw numbers. When a numeric expectation fails, check the semantics first; only then copy the number.

## 2026-09-30: never attribute a metric change without diffing the edge sets
- **Mistake:** hono's resolved share fell from 14.8% to 13.5% after a fix, and I wrote in the changelog that middleware `next()` calls were the cause. The evaluator diffed the edge sets: all 329 lost edges were calls to `const f = () => …` functions that my fix had wrongly suppressed. I had described a regression as an improvement.
- **Rule:** When a measurement moves, diff the before/after edge sets (or sites) and classify the difference before writing a sentence about why. A change without a classified diff is reported as "changed, cause not yet classified".

## 2026-10-01: measure an image before pulling it, and keep `target/` from eating the disk
- **Mistake:** I pulled `scip-java:0.13.1` knowing the disk had 4.8 GiB free and without a size: the image is 6.75 GB uncompressed. It filled the disk, broke the second pull with an I/O error and stopped cargo from writing. Our own `target/` had also grown to 12 GB from release and worktree builds nobody pruned.
- **Rule:** Before any pull, get the uncompressed size (`docker manifest inspect -v`, summing each platform's layer sizes, or the registry's page) and check `df`; never pull anything bigger than half the free space. Run `cargo clean` or set `CARGO_TARGET_DIR` under the scratchpad for one-off builds (worktrees, measurements), and check `du -sh target` when a session runs long.

## 2026-10-01: a rule that suppresses a fallback must count what it suppressed
- **Mistake:** invariant 1 ("a SCIP-ok project gets no heuristic edges") silently dropped every edge of the 42 files class-validator's tsconfig excludes from the index: 927 call sites, 70% of the heuristic twin's, gone with no diagnostic. The fixtures never showed it because their tsconfigs include every file.
- **Rule:** whenever a rule turns a fallback off for a set of inputs, count the inputs it covers (`files_without_document`) and print the count next to the coverage it replaces. Measure a public repo whose build excludes files before calling the coverage report complete.

## 2026-10-01: a relationship flag does not carry direction; check it against syntax
- **Mistake:** I turned every `is_implementation` relationship into `A IMPLEMENTS B` with A the symbol that carried it. scip-java sets the flag on both sides of an override, so the interface method "implemented" its implementation and `rb impact` listed the interface as a breaking dependant.
- **Rule:** An indexer's relationship is a link, not an arrow. Before an edge kind with a direction is derived from one, confirm the direction against a fact the syntax states (`extends`/`implements` on the source side) and test with a symmetric input, so the test fails if the direction is guessed.

## 2026-10-01: a test that reads a fake process's log must not assume when the process started
- **Mistake:** the timeout test asserted that the fake `docker kill` line was the log's *last* line. Under the full workspace run the fake's shell started after the 300 ms budget, its `run` line came second, and the test failed only under load.
- **Rule:** When a test spawns a process and checks a log, assert on the presence of lines, not their order, unless the order is what the test proves; and keep the budget an order of magnitude above a process start.

## 2026-09-30 (wall clock): a rule that walks a relation must be tested past depth one
- **Mistake:** the `IMPLEMENTS` direction rule looked at the holder's direct supertypes only. scip-java relates an override to the method it implements however many levels up, so every hierarchy deeper than one level lost its edge; my only test had one level.
- **Rule:** when a rule follows a relation (supertypes, imports, callers), test depth 1, depth 2 through a class and through an interface, and the reverse at each depth. Read what the indexer emits from a real index (dump its relationships) before choosing the rule, since that is the case the reviewer tries next.

## 2026-09-30 (wall clock): anything named by its inputs is shared by concurrent runs of those inputs
- **Mistake:** the work directory and the container names were the input hash, so two runs of one commit deleted each other's export, collided on `--name`, and cached the failure.
- **Rule:** content-addressed names are for results (cache keys). Anything a run mutates (work directories, container names, temp files) gets a per-run instance id. Test it by running the same job twice and asserting the runs share no path and no name.

## 2026-09-30 (wall clock): a counter defined by exclusion hides distinct cases
- **Mistake:** `external` meant "not defined in the index", which also caught the project's own generated Prisma client and a record's compiler-generated accessors, so the status line over-reported dependencies on two fixtures and nobody had looked at what the 4 were.
- **Rule:** name a counter by a positive property, and when a golden with counts is first blessed, identify one member of each non-zero bucket in the index dump.

## 2026-09-30 (wall clock): check the images and the runtime before an e2e that runs containers
- **Mistake:** I started the SCIP e2e without `docker image inspect` on the images it uses. The scip-java image was absent, so two tests printed `skipping:` and counted as `ok`; free space fell from 3.6 GiB to 0.1 during the run (cause not classified: the runtime stopped answering before I could list images), and at zero the harness could not write its own output files.
- **Rule:** before the e2e, run `docker image inspect` for every image the jobs use (absent means a skip, or an implicit pull by `docker run`), `docker context ls`, and `df`. Report a test that printed `skipping:` as not run. Keep 2 GiB free as a floor: check `df` after each build and each container step, and clear old sessions' artifacts from the scratchpad at the start.

## 2026-10-01: output that passes through a shared cache is not a function of the input
- **Mistake:** two assertions depended on shared-cache state: the TypeScript status golden embedded `4.1s` from the database's cache row (a fresh database would fail it), and my check for Maven's error text matched only the first miss (`Could not find artifact …`), while a repeat against the shared `.m2` says `was not found … during a previous attempt`.
- **Rule:** Before asserting on text that crossed a cache (`.m2`, `~/.npm`, `rb.scip_index`), run the case twice, cold and warm, and diff. Mask what differs (run times) and assert on the part that stays (the resolution summary); a golden must not need a specific cache row to pass.
