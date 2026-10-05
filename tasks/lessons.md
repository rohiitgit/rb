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

## 2026-10-01: a claim about file modes or mounts holds only for the runtime it was tested on
- **Mistake:** amendment 8 said an exported file is never executable, so scip-java could not run `./mvnw`; that was true of colima's mounts and false on Docker Desktop's virtiofs, where `access(X_OK)` answers yes for root, so every Maven repository with a wrapper failed.
- **Rule:** never rely on a mount's permission semantics; remove the hazard from the input (do not export the wrapper) and re-run the public-repo measurement on the runtime of record after any runtime change.

## 2026-10-01: simple-name resolution is never exact
- **Mistake:** the `IMPLEMENTS` walk resolved a declared supertype by its simple name to every project type with that name, so a second `Common` anywhere in the project turned scip-java's reverse override link into an exact edge.
- **Rule:** inside an exact tier, resolve names through the indexer's own occurrences (or not at all); a name-based fallback belongs to the heuristic tier and must be labelled so. Test with two same-named types where only the wrong one leads to the target.

## 2026-10-01: wall-clock time never enters the graph; mask nothing in goldens
- **Mistake:** `elapsed_ms` sat in the `scip_status` detail, so two fresh runs differed and the e2e grew a mask (`mask_elapsed_ms`, `<elapsed>` tokens) to hide it instead of removing the cause.
- **Rule:** a value that needs masking in a golden is a determinism bug; print run times from the run result and store none. When a byte cannot be made a function of the commit (a build tool's stderr), compare without that field and say why in the contract.

## 2026-10-01: a span rule must name exactly what it covers before anything resolves through it
- **Mistake:** `Supertype.span` covered the whole clause item (`Marker<Marker.Disc>`, `@Tag Shape`), and the join picked the "longest extending" type occurrence in it, so a nested type argument or an annotation became the supertype and one false exact edge appeared.
- **Rule:** a span used for resolution covers one named thing (the type's name, not its clause); state it in the field's doc and test it with an argument, a qualifier and an annotation on the same line before any rule resolves through it.

## 2026-10-01: an "up to date" decision must look at what the last run failed to do, not only at its configuration
- **Mistake:** `rb index` compared `(commit, tool_versions)`; an `environment` or `timeout` job (not cached, "decided again next run") froze into a heuristic snapshot that every plain run accepted.
- **Rule:** whenever an outcome is deliberately not cached, the thing that holds it (the snapshot) must not count as current either; derive the check from the stored outcomes and test "transient failure, then a plain run" end to end.

## 2026-10-01: never undo a temporary edit with `git checkout --` in a tree that has uncommitted work
- **Mistake:** I reverted a one-line trace in `scip.rs` with `git checkout -- crates/rb-indexer/src/scip.rs`, which discarded 352 uncommitted lines of the fix; they were recovered from the diff recorded in the session transcript.
- **Rule:** undo a temporary edit by reversing that exact edit (the same script with the strings swapped, or `git stash` of the trace alone); `git checkout --`/`git restore` only on a file whose `git diff` is empty before the temporary edit.

## 2026-10-01: an indexer's column convention is measured on the real indexer, not inferred from its docs
- **Mistake:** the contract noted "scip-java corrects leading tabs only; interior tabs can shift columns" and nothing measured what the shift was, so a heritage clause with an interior tab produced a false exact edge, and jsoup lost six definitions to a `<spaces><TAB>` indent.
- **Rule:** when a fact says an indexer's positions can be off, build a scratch project with every layout (leading, interior, mixed, inside literals, non-ASCII) and derive the rule from its real output before trusting any span from it; keep the measured lines as a unit test.

## 2026-10-01: "defined in the project" needs one operational definition before a counter is promised
- **Mistake:** a counter was specified as "both ends defined in the project", which reads differently for an anonymous class's method (the index defines it, the syntax deliberately does not); jsoup gives 0 or 52 depending on the reading.
- **Rule:** before adding a coverage counter, write its membership test as code-level facts (which occurrence, which Tree-sitter definition), run it on a public repo, and record both what it counts and what it deliberately leaves out next to the number.

## 2026-10-01: line numbers from another tool use that tool's line terminators
- **Mistake:** the decoder split a document on `\n` to read an indexer's line numbers, as Tree-sitter counts rows; javac also ends a line at a lone `\r`, and TypeScript at U+2028/U+2029, so one such character shifted every later occurrence of the file onto another token and gave definitions other methods' keys.
- **Rule:** never index another tool's `(line, column)` by your own line split. Convert through the byte offset: the tool's line starts (measured on its real output, with lone CR, CRLF, U+2028/U+2029/U+0085 in strings and comments), then your own rows. Measure lines and columns together; amendment 53 measured only columns.

## 2026-10-01: a consistency guard is calibrated on real indexes before it is trusted
- **Mistake (avoided by measuring first):** "the symbol's name equals the token" sounds exact, but strict equality would have rejected 2,137 correct occurrences on four public repositories (constructors at their type's name, `validate0:` properties, accessors, default-import aliases) and silently dropped their edges.
- **Rule:** before adding a guard that rejects data, run it as a trace over the fixtures and real repositories, enumerate every rejection, encode the legitimate forms, and require 0 rejections on clean data; keep a unit test on a deliberately misaligned index so the guard still bites.

## 2026-10-01: when two parsers must agree on positions, check agreement per document and fall back whole-file
- **Mistake:** passes 5–7 each patched one more way javac/TypeScript and Tree-sitter read a file differently (tab columns, line terminators, then source encodings and comment terminators), and each pass found another class that produced false exact edges.
- **Rule:** when exactness depends on two independent parsers reading the same bytes alike, add a per-unit agreement check (here: per document, signs in the text plus "every definition the indexer places is accounted for by the syntax") and fall back to the lower tier for the whole unit, with a counted, per-reason rejection. Calibrate the check to 0 rejections on clean real data first, then patch individual lexing differences only where the fallback's coverage loss is measured to matter.

## 2026-10-04: resuming someone else's uncommitted work, verify records against code
- **Mistake:** the predecessor wrote the contract amendment (reason order "as listed") before the code settled; the code applied `source_encoding` after the decoder's reasons, so the record and the behaviour disagreed. A scripted edit had also moved one function's doc comment onto another.
- **Rule:** when resuming, read the whole diff, check each claim in the already-written records (order, counts, test names) against the code, and re-run every mutation and measurement the records cite before relying on them; check that no mutation backup (`mut.bak`) differs from the file it was taken from.

## 2026-10-05: an agreement check must compare what the edge is made of, not a proxy for it
- **Mistake:** pass 7's backstop (`definition_disagreement`) compared only where the indexer placed *definitions*, but an exact edge is made of a reference and its *source*; restructurings that moved only references (a `*/` that ends a comment for javac only, an encoding that shifts only the calls on a line) passed it, and amendment 64 called those classes impossible.
- **Rule:** check agreement on the exact fact the output is built from (here: each edge's source against the indexer's own enclosing ranges), measure first which of the indexer's fields carry that fact and in which units, and never write "impossible" in a record: list each class with the check and the test that cover it, and the vectors still uncovered.

## 2026-10-05: when configuration cannot be read completely, let the data decide
- **Mistake:** the source encoding was read from literal pom elements; `.mvn/maven.config`, `compilerArgs -encoding` and a module's property override all set it elsewhere and gave false exact edges.
- **Rule:** keep the static reading as a cheap first signal, but add a check on the output itself that does not depend on knowing the configuration (here: read each position under both encodings and reject when the positions cannot tell them apart); calibrate it to 0 on real repositories and record any genuinely ambiguous file it rejects.

## 2026-10-05: an agreement check between two structures must run both ways
- **Mistake:** pass 8's structure check compared each edge with the ranges of the definitions the join had *anchored*; a member only javac saw had no anchor, so its range was passed over like a field's and its calls became exact edges from the enclosing class, and a method only Tree-sitter saw was never compared at all.
- **Rule:** when two parsers' structures must agree, check inclusion in both directions (every definition of each side has a partner on the other, modulo forms measured on real repositories to be deliberately uncaptured), and let a later check consider every element of the other side, not only those already matched; then measure which unmatched forms occur on clean data before exempting any.

## 2026-10-05: scratch repositories inside the project are committed with `git -C <absolute path>`; escape-bearing text is generated
- **Mistake:** a relative `cd ../../../../..` from a scratch project's source folder landed in `var/fix-pass9`, inside the project repository, and `git add -A && git commit` there committed the uncommitted fix (undone at once with `git reset --soft HEAD~1 && git reset`, working tree untouched). Separately, `\u002a` typed into a shell heredoc reached the file as `*`: the tool input decodes `\uXXXX` before the shell sees it.
- **Rule:** run git in a scratch repository only as `git -C /absolute/path/to/scratch …`, and check `git rev-parse --show-toplevel` first when in doubt. Write any text that must contain a literal `\uXXXX` through a script that builds the backslash (`chr(92)`), and check the bytes (`od -c`) before using the file as evidence.

## 2026-10-05: an exemption written as a structural shape must be checked against every way the shape can arise
- **Mistake:** amendment 54 bound any same-named local to a Tree-sitter definition because enum constant bodies needed it, and the first restriction proposed (innermost method/type range is the parent's) still admitted a field initializer's anonymous class and an initializer block's local variable, each a false exact edge on the real image.
- **Rule:** when an exemption exists for one legitimate form, identify that form positively with every fact the indexer gives (kind of the symbol, kind and owner of the innermost enclosing definition), measure that all real instances match it (jsoup 117/117), and build one counter-layout per nesting the rule might also accept (method, field initializer, initializer block, nested local) before trusting it.

## 2026-10-06: when one input trick keeps producing findings, reject the trick at the input
- **Mistake:** passes 8–11 each found another layout in which a Unicode escape javac alone translates restructured a Java file (a moved reference, a hidden member, a falsely paired local, a supertype clause), and each pass hardened one downstream structural check; the fifth layout still got through the one allowance left.
- **Rule:** when the findings of several passes share one input mechanism, measure how often that mechanism occurs on real data (here 0 documents on seven repositories once three harmless characters were left out) and reject it at the input; keep the downstream checks as a second line. Do this the second time the mechanism appears, not the fifth.

## 2026-10-06: a check that relies on an indexer's metadata is measured for every indexer and every kind
- **Mistake (avoided by measuring first):** "confirm each declared supertype by the type's own relationships" holds for scip-java, which relates every type to all its supertypes, but scip-typescript relates interfaces to nothing; the literal per-step rule would have dropped true `IMPLEMENTS` through every TypeScript interface.
- **Rule:** before requiring an indexer field, build a scratch project with each form (class, abstract class, interface, interface chain, external and namespaced supertypes) for each indexer, read the raw output, and state the rule in terms of what every indexer actually emits; record any deviation from the decided wording with the measurement that forced it.

## 2026-10-06: an input rule's set is derived from both lexers, meta-characters included
- **Mistake:** amendment 77's escape set listed the characters that delimit structure (`/ * { } ( ) ; , " ' = < >`, line terminators) and a unit asserted the backslash harmless; but a backslash changes how the *next* character lexes inside a literal, so javac's produced backslash moved a quote that Tree-sitter treated as closing (pass 12, O1).
- **Rule:** when rejecting an input trick at the source, derive the set by asking, for every character the trick can produce, what each parser does with it *and with the characters after it* (escape, quote, comment and terminator roles), then measure the set on real data. Stop the review loop when a pass's only major is a gap in such a derived rule and the fix is calibrated at zero cost; record any mutant the fix makes equivalent instead of contorting the code to keep it killable.
