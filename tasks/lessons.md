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

## 2026-10-06: a mutation run that printed nothing has not run
- **Mistake (caught before recording):** two mutation batches reported every mutant "not killed" or printed nothing at all: the first wrapped `cargo test` in `timeout`, which macOS does not have, and the second passed `cargo`'s arguments as one unquoted `$5`, which zsh does not word-split, so `cargo` was asked for a package named `rb-store --lib impact`. Both would have read as surviving mutants (or as passing guards) had I trusted the summary line.
- **Rule:** a mutation harness prints the test runner's own result line for every mutant (`test result: …`), and a mutant with no such line is "not run", never "not killed". Pass argument lists as `"$@"` and check that every tool the harness calls exists (`command -v`) before the batch.

## 2026-10-06: a ranking rule designed on exact matches must be re-derived for every match kind it ranks
- **Mistake:** D9 ("most literal segments win") was written for exact matches and applied unchanged to suffix matches, where the client matches only the endpoint's tail. On spring-petclinic-microservices it sent 3 of 5 suffix calls to the endpoint with the longest path instead of the one whose tail the call names; the fixtures never showed it because their one suffix site had a single candidate.
- **Rule:** when a rule orders candidates, restate it for each kind of match it orders (exact, suffix, wildcard) in terms of the part that actually matched, and give each kind a unit with two candidates where the whole-path reading and the matched-part reading disagree.


## 2026-10-06: a node key must denote one thing, including when part of its identity is unknown
- **Mistake:** endpoint keys were `(method, path, service)`, and a route whose prefix was unknown kept only its tail in the path, so on gin-realworld four nodes stood for two to three distinct routes each (`GET /` = articles, tags and user); the fixtures had no unknown prefix, so only a public repository showed it (fork F3).
- **Rule:** when a component of a natural key can be unknown, replace it in the key by a stable anchor of the unknown (here the router's file and scope-qualified binding, never a position) instead of dropping it, and test with two inputs that differ only in the unknown part.

## 2026-10-06: count test results from a log that holds both streams
- **Mistake (caught before recording):** `cargo test > log 2>&1` was written as `2>&1 > log`, which sends stderr (where cargo prints `Running …`) to the terminal; the awk sum keyed on `Running` lines then counted every `test result` line, the e2e suite's 17 included, and gave 436 instead of 419.
- **Rule:** redirect as `> file 2>&1`, and check that the per-crate lines the sum is built from name every binary (`unittests`, `tests/e2e.rs`) before trusting the total.

## 2026-10-06: a gap list that says "never a false X" must also say which false Y remain
- **Mistake:** M4a's open items described every gap as "a missed or less precise result, never a false `synthetic`". That was true and hid false `heuristic` stitches: evaluator pass 1 found one on a public repository and three from common idioms (a port hole, a query tail, a suffix tail of only parameters), plus a silently incomplete `rb path` answer.
- **Rule:** when a record bounds the worst failure ("never a false `synthetic`"), it also lists the weaker false results still possible, each with the input shape that produces it and its measured count, so the bound cannot be read as "no false edges".

## 2026-10-06: a visited set over a scoped node is keyed by the scope
- **Mistake:** `rb path` kept one visited set of nodes, but an inline-handler `File` is walked within one endpoint's spans at a time; the second endpoint handled in the same file found the file "seen" and lost its downstream, with nothing in `collapsed` or `truncated`. The same keying dropped an upstream continuation.
- **Rule:** when a walk restricts what it follows from a node (spans, a scope, a filter), key its visited set by (node, restriction), and test two restrictions of one node in both directions.

## 2026-10-06: a value a library merges from several places is read from all of them, or is unknown
- **Mistake:** the axios pack read `baseURL` from the instance and from `axios(config)`, but not from a verb call's request config, which axios merges over the instance defaults; `api.get("/orders/1", { baseURL: "https://api.stripe.com" })` became a false `synthetic` edge to the instance's service. `RestTemplate` ignored a visible `rootUri(…)` the same way.
- **Rule:** before trusting a configured value, list every place the library lets a caller set it (instance, call arguments, setters, builders) from its documentation, read each, and make "present but unreadable" unknown rather than falling back to another source; test one call per place with a contradicting value.

## 2026-10-06: a record that says a rule applies must be backed by a test of the rule's effect, not of the field
- **Mistake:** amendment 11 said a host with a hole gets "the unresolved-base rules"; the unit checked `host_dynamic` and the path, but nothing set the base unknown, so the stitcher matched the path exactly across every service (`https://${tenant}.zendesk.com/…`).
- **Rule:** when a record names the consequence of a rule (suffix only, capped, external), the test asserts that consequence at the stage that applies it (the stitcher's bucket and `match`), from source in every form the record lists.

## 2026-10-06: growth tests count operations, they do not time them
- **Mistake:** four ratio tests compared two wall-clock runs of tens of milliseconds; a load step covering one of them gave 2.5–3.4× (7 of 20 runs of one test failed under intermittent load), and best-of-N did not help.
- **Rule:** a complexity claim is tested by counting the elements the measured loops examine (a test-only counter) at n and 4n and bounding the ratio (5× for linear); show it bites by making one loop quadratic. Keep wall-clock only as a coarse bound on a pathological input.

## 2026-10-06: a value folded from a declaration is certified only by a scan a shadowing name cannot fool
- **Mistake:** M4a read a client's base from its construction (`axios.create({ baseURL })`, a builder's `baseUrl(…)`), and three evaluator passes each found more statements that change it afterwards (a request config, `defaults.baseURL`, an interceptor, a reassignment, a spread, a mutated builder, an importer); pass 3's answer, "prove every use leaves it alone", was built on resolved uses, and pass 4 found the resolution blind to uses written before the declaration, to Java fields reached through any other reference, a subclass, a generated getter or another file, and to an aliased export: each a false `synthetic`, and the records claimed "every use".
- **Rule:** certify a value for the highest certainty only with a check whose failure modes all point down: scan every token of the binding's *name* over its whole scope (the enclosing function, or the file; before the declaration too), require exactly one declaration and only known-harmless uses, and never certify what other code can reach without that name (fields, exported or imported values). Write down what the scan cannot see (code from strings, reflection, instrumentation, global library hooks) and what it costs (here: every field-held Java client and exported TS instance stays `heuristic`), with counts. Anything not proven lowers the certainty and keeps the value.

## 2026-10-06: gate what carries the effect, not where one input came from
- **Mistake:** Gate A certified a client's *base* (it ran only when a base had been read from an instance's construction), so everything else that decides where a request goes went unread: the copy a request was sent through (`w.mutate().filter(…).build().get()`), an interceptor or filter on an instance whose base was never read, a Java field holding a client with an absolute URL, an axios `adapter`. Evaluator pass 5 found each as a false `synthetic`, and the contract's own text ("every call on an unverified instance") was not what the code did.
- **Rule:** when a certainty depends on a value reaching its destination unchanged, list every object on the path from the value to the effect (the instance, any copy, the request's own options, module-level defaults, process-wide hooks) and require each to be proven plain; never key the check on the presence of one input. Write the categories that may pass as an allow-list, so a carrier nobody thought of is capped by default, and give each guard its own biting row (two guards that each catch the same repro mask each other's mutants).

## 2026-10-06: a deny-list of reconfiguration forms is never complete; allow-list the roles a name may appear in
- **Mistake:** amendment 15 decided whether a client was plain by listing the ways it could be reconfigured (`globalThis.fetch = …`, `axios.defaults…`, `http.DefaultTransport`, `new RestTemplate(args)`), and evaluator pass 6 found five spellings the lists missed: a cast (`(global as any).fetch = jest.fn()`), a bare assignment, a function-local `require("axios")`, instances inheriting `axios.defaults`, a Go dot import, `RestClient.create(restTemplate)`, a Feign `RequestInterceptor` — each a false `synthetic`.
- **Rule:** to certify that something is unconfigured, record every token of every name that reaches its configuration surface (in every syntactic position, property and type names included) and allow-list the few roles that leave it alone (a call, a request receiver, its own import); everything else taints. When a new form is found, ask which allow-listed role it slipped through, never add it to a list of bad forms. Write down what a token scan cannot see by construction (computed names, reflection, string evaluation, instrumentation) as the residue, with examples.

## 2026-10-06: stop a harness that edits sources so that it restores them, and check that it stopped
- **Mistake (caught, no damage):** a mutation harness that writes a mutant, runs the tests and restores in `finally` was stopped with `pkill -f "mut/harness.py"`, which matched nothing (the process ran as `python3 harness.py` from that directory), so it kept mutating while the source was being edited; a second kill of the test process let it move on to the next mutant. SIGTERM would also have skipped the `finally`.
- **Rule:** stop such a harness with SIGINT (Python runs `finally` on `KeyboardInterrupt`), confirm with `pgrep` that it is gone, then check that every mutant's original text is back (each `old` string present exactly once) before touching the tree; never edit sources while a harness runs.

## 2026-10-06: a guarantee over "the repository" names the files its mechanism reads
- **Mistake:** amendment 16 said a carrier is plain only if every token naming its configuration surface "anywhere in the repository" is allowed, but the tokens came from parsed files only; evaluator pass 7 put the same hooks in a Kotlin bean, a Vue component, an HTML page, a generated file and a 1 MiB bundle, and each stayed a false `synthetic`.
- **Rule:** when a claim quantifies over the repository, list which files the mechanism actually reads (parsed, by language, size and generated marker) and give every other file a conservative fallback (here a raw-text word scan that can only over-taint, with a byte cap that taints); test the file-selection step itself (parsed, skipped, binary, over the cap), not only the scan.

## 2026-10-06: measure a conservative fallback on the fixtures before trusting its cost
- **Mistake (caught before building):** the decided raw-text scan of "any repository text file" would have capped the fixture's own axios and `fetch` hops through a `package.json` dependency entry and the word "fetch" in a `pom.xml` comment, and every axios repository through its lock file: a soundness fix that silently removes the result it protects.
- **Rule:** before adding an over-tainting fallback, run its matcher over the fixtures and the public repositories and read every hit; exempt only text that no runtime executes or reads as configuration, name each such class with the reason (prose, lock files, plain registry dependency entries, XML comments), give each exemption and each non-exempt neighbour (an alias, another field, MDX) its own test row, and state the deviation from the decision.

## 2026-10-06: state the soundness bar before adversarial review starts, and close on verification
- **Mistake:** M4a's carrier soundness had no stated bar until amendment 16 ("no false `synthetic` from any form a repository-wide name-token scan can see"). Evaluator passes 3–7 each found new reconfiguration forms, 16 blockers in all, because "never a false `synthetic`" admits an unbounded supply of constructed code, and each fix invited the next probe. The milestone converged only once the bar was named, the residue was listed, and amendment 17 made the closing pass verification-only, with open-ended probing moved to M5's adversarial carrier review.
- **Rule:** when a guarantee is adversarial ("never false X"), write its bar in the contract before the first evaluator pass: which forms are in scope and which residue is out by construction. Then decide up front how the milestone closes: a verification-only re-check of the listed findings, with new attack forms recorded for a named hardening item and not for the verdict.

## 2026-10-06: a fixture plan is code; check it against the language before the fixture phase
- **Mistake (caught at the first edit):** the M4b fixture plan put `public interface PasswordHasher` in `Hashing.java` and `public interface Notifier` (implemented from another package) in `Notifiers.java`; javac rejects a public top-level type outside its own file, so the SCIP fixture would have failed its build.
- **Rule:** when a contract specifies fixture files, read each against the language's file and visibility rules (and the indexer that must compile it) before writing them; record the deviation as an amendment, not as a silent rename.

## 2026-10-06: a match by simple name needs every part of the written name to agree
- **Mistake:** amendment 8 matched an external injected type to a repository `@Bean` method by simple name, so `ChatClient.Builder` resolved to a `WebClient.Builder` factory on a public repository: a false `heuristic` edge no fixture showed (the fixtures have no nested external types).
- **Rule:** whenever a rule compares type names as written, compare as much of the name as both sides wrote (a dot-separated suffix), test nested types with the same simple name, and sample the public repositories' edges of that origin before closing.

## 2026-10-06: a mutation log that de-duplicates lines across tests can hide a bite
- **Mistake (caught before recording):** the e2e harness printed each failing guard line once per mutant, so `INJECTS * -> * [synthetic]` failing in two fixtures appeared once; it looked as if only one fixture's guard had bitten.
- **Rule:** when a harness summarises failures, key the de-duplication by test (or fixture) and line, and confirm a guard shared by several fixtures from the raw log before writing which ones bit.

## 2026-10-07: a role allowed "because a later rule handles it" must be checked against what that rule actually sees
- **Mistake:** RTA allowed a name's `supertype` mentions because rule (i) propagates instantiation from subtypes, but (i) walked only definitions of the facts; a local class in a lambda or a class expression mentions its base as a supertype and is no definition, so its instances never reached the base (M4b evaluator pass 1, Y2). The same pass found the orphan rule (e) reading only the declaring class's own supertypes while its stated reason ("an override of an external method") needs the whole chain (Y3).
- **Rule:** when an allow-list admits a role on the grounds that another step covers it, enumerate every producer of that role and check that each one is an input of the covering step; give a row for a producer the step cannot see. A rule whose reason is transitive ("outside the repository", "instantiated") walks the chain, with a visited set and a conservative cap.

## 2026-10-07: a tie-breaker never runs past a filter that is not modelled
- **Mistake:** DI applied `@Primary` (and name fallback) after narrowing by the filters it modelled, but Spring applies generic type matching and custom qualifiers first; with those unmodelled, `@Primary` chose a bean Spring never considers (Y5, Y6, Y13), and the edge only carried `capped_by`.
- **Rule:** a tie-breaker (primary, name, order) may only decide among the set the real resolver would have left; when any filter before it is unmodelled for the input at hand, keep every candidate (`ambiguous`) instead of picking one, and test the tie-breaker with two and with three candidates.

## 2026-10-07: a name scan reads names as the runtime writes them
- **Mistake:** RTA's word splitters treated `$` as a word character, so a nested class written as its binary name (`Holder$LoadedImpl` in `Class.forName` or `META-INF/services`) never mentioned `LoadedImpl` (Y1); and its text scan skipped Python, Go and `.txt` files (Y14).
- **Rule:** before trusting a "never mentioned" scan, list how the runtime spells the name (binary, qualified, path, file-name forms) and split words so that each spelling yields the simple name, and read every file the runtime may read; when a precise mode's proof has a known gap, make the conservative mode the default (RTA became opt-in).

## 2026-10-07: keep a runtime harness out of the repository the indexer reads
- **Mistake (caught while verifying):** the first DI repro of pass 2 put the Spring context bootstrap (`new AnnotationConfigApplicationContext(App.class)` in `main`) inside the indexed repository; that is a programmatic registration surface, so gate D capped every point (`surface`) and hid the gate-M behaviour under test.
- **Rule:** when a finding is confirmed at run time, compile the repro's sources together with a harness class kept outside the indexed tree; index exactly the shape the finding describes, and if the harness must live in the repository, check first which gates its own code trips.

## 2026-10-07: a test that loops until the system says "stop" bounds its own iterations
- **Mistake:** the pagination e2e followed `next_offset` until it was `null`; a mutant that dropped the CLI's `--offset` made every page the first one, so the loop never ended and the mutation harness hung for ten minutes until it was stopped with SIGINT (the source restored and checked).
- **Rule:** any test loop driven by the system under test (pages, retries, polling, a fixpoint) asserts progress at each step (the page starts where asked, the next offset is strictly larger) and runs at most a bound computed from the input (`total` pages), so a regression fails instead of hanging.

## 2026-10-07: splitting a cache re-exposes the tools' own bookkeeping in it
- **Mistake (caught by the e2e, fixed before committing):** per-repository package caches gave every fixture an empty npm cache, and npm's update notifier, which records its last check there, printed a "new version available" notice on the first fetch and not on the next; a failed fetch's stderr, which the dump stores, then differed between two runs of one commit. The shared cache had hidden it for months because its notifier timestamp was always fresh.
- **Rule:** whenever a cache is split, moved or emptied, run the determinism checks again from a cold cache and a warm one, and turn off every tool behaviour that depends on cache state and can reach stored output (update notifiers, "previous attempt" failure caches, telemetry banners) explicitly in the recipe.

## 2026-10-07: an allow-list the adversary can extend is no allow-list
- **Mistake (caught while designing, amendment 12):** the contract let the fetch phase's registry allow-list be extended by `rb.toml`, which `rb` reads from the repository being indexed; the repository's own build is exactly what the egress guard contains, so it could have allowed its own exfiltration host.
- **Rule:** for every input to a security control, ask who writes it; configuration that widens a guard comes from the operator's side (flags, host environment), never from the data the guard is applied to, and is validated in the form the guard matches literally.

## 2026-10-07: a bulk load is not readable until the planner's statistics cover it
- **Mistake:** the publish COPYed a whole snapshot and committed, leaving the statistics to autovacuum. For up to a minute the planner believed the new `snapshot_id` had one row and planned a nested loop over the whole snapshot: a read that takes 1 s ran for over 10 minutes. The fixtures never showed it (tiny tables, plans cheap either way), and neither did warm benchmark runs (autovacuum had caught up).
- **Rule:** a transaction that bulk-loads rows a reader will filter by a new key ends with `ANALYZE` of those tables before it commits, so the statistics become visible with the rows. Measure the first read right after a publish, not only warm reads, and test the planner's estimate for the new key.


## 2026-10-07: a disk figure is a sum of named parts, or it is not a record
- **Mistake (found in phase 20):** phase 19 recorded "17 GiB free at the start of phase 16, 13 GiB at the end" and named 1.3 GiB that remained (one of the two parts over-stated: `var/m5/bench/out` is 402 MiB, not 0.6 GiB), while its own verify line ("disk back above its starting level") was ticked. The unnamed 2 GiB were two 1 GiB macOS swap files created during the shardingsphere SCIP run, on the same APFS volume, outside the project.
- **Rule:** when a step's verify compares free disk, record the before and after in MiB and account for the difference part by part with `du` (project directories, Docker's VM image, `/System/Volumes/VM` swap files by birth time); if the parts do not add up, say how much is unexplained and do not tick the check.

## 2026-10-07: every commit point of a multi-transaction operation is a crash point, and readers must not trust what lies between
- **Mistake (evaluator pass 1, Z2):** the publish flips in one transaction and drops the old AGE graph in the next. The property test drove publishes, failures and retention, but never a publisher that dies between the two commits; a `SIGKILL` there left a retired snapshot still naming its graph, and `rb cypher --snapshot` answered from it, because Cypher checked that the graph existed, not that the snapshot was live.
- **Rule:** list the commit points of every operation that spans transactions and give each one a test that stops right after it (a test hook, and once without a hook: a real kill while a lock holds the next step). Readers decide from the state row (`status = 'live'`), never from whether an artifact still exists, and the leftover a crash can leave gets a cleanup that runs on its own (the next run, a startup sweep), not only a manual command.

## 2026-10-07: a test that sends more than the server reads may see a reset instead of the reply
- **Mistake (caught while writing the Z7 test):** a 420,078-byte header was answered `200` (hyper read it whole although its buffer limit is about 410 KiB), and a 1 MiB header was `431` only because the server closed before reading the rest, which a client can see as a connection reset instead of the reply.
- **Rule:** a transport-limit row in a test must be one the server reads whole before refusing (too many headers, a long target); a refusal that closes mid-request is recorded as a measurement, not asserted, and a limit is never assumed from a constant in the source without sending the request.

## 2026-10-07: a claim about "every" item needs a test with at least two, in a state the test controls
- **Mistake (evaluator pass 2, N2):** `rb serve`'s startup sweep claims every repository, but `sweep_graphs` was tested with one repository; the mutant that sweeps only the first name survived the `rb-store` suite and was killed by the e2e only because the evaluator's own repositories happened to sort before the test's.
- **Rule:** when a mechanism loops over a collection, test it with at least two members whose order puts the interesting one last, inside state the test creates; a kill that depends on what else is in a shared database is not a kill.

## 2026-10-07: a test step that depends on a side effect of a run must not assume the run happened
- **Mistake (caught by the verify loop, M6a phase 8):** `scip_ts_workspaces` deleted the per-repository package cache with `remove_dir_all(...).unwrap()` before its cold fetch. Alone it passed (the first index ran a container, which created the cache); in the full suite the first index was served from the database's index cache (rows outlive test runs), no fetch ran, the directory did not exist, and the test panicked.
- **Rule:** when a test manipulates state that only a non-cached run creates (a package cache, a work directory, a container), make the step idempotent (`exists()` before removing) and force the run that must happen (`--no-cache`), then run the test once warm and once from an emptied database cache before blessing.

## 2026-10-09: a background bench run must outlive the tool call that started it
- **Mistake:** a ~31 min `rb index` was started in background with `&` at the end of a command whose tail (`sleep 5; cat …`) kept the tool call alive past its 60 s timeout; the timeout killed the whole process group, the run died 90 s in, and its watchdog exited silently (its `.armed` file already existed). A relaunch attempt then failed invisibly (`setsid` does not exist on macOS) and its watchdog exited at once (stale `.armed`).
- **Rule:** start long runs with `nohup … & disown`, delete any stale guard files (`.armed`) first, and return from the starting call immediately (no trailing sleep); verify liveness from a later call (`tail` of the watch log, `ps`, container list). Never let a starting call run into its timeout. Check for the platform's detachment primitives (`setsid` is Linux-only) before relying on them.

## 2026-10-09: an indexer's marker behaviour is per version, measured per version
- **Mistake (caught by measuring):** the Lombok marker rule was probed on 1.18.34 only; the second measurement repository resolves 1.18.30, whose index holds zero `lombok/Generated#` occurrences, so D1 removed nothing there. Had the phase assumed "Lombok marks", the zero would have read as a removal bug.
- **Rule:** when a rule depends on a tool's emitted metadata, check the metadata's presence in every measured index (bytewise symbol search of the cached row) before interpreting a zero count, and record the producing versions (here: 1.18.30 unmarked, 1.18.34/1.18.44 marked). A zero with absent metadata is the residue case, not a failure.

## 2026-10-09: a cached failure replays; a second fresh execution needs `--no-cache --force`
- **Mistake (caught before recording):** the second threaded shardingsphere run finished in 24 s with the first run's failure: `failed` outcomes are cached (only `stderr_tail` may vary), so a plain rerun proves the cache, not the build. Determinism of the failure itself needed `--no-cache --force` (22 min), which reproduced it row-for-row.
- **Rule:** when the claim is "two fresh runs agree", force both past the cache (`--no-cache --force` into distinct repositories); a cache-hit rerun only proves replay. Name which run was fresh in the record.

## 2026-10-09: host load is part of the disk profile, not just headroom
- **Mistake (measured):** the Oct-7 shardingsphere fetch died at the 6.1 GiB floor from 16.4 GiB under load 43; the identical run from 16.1 GiB under load ~4 completed with the floor never below 10.5 GiB. Free space alone did not explain the first death (macOS swap under memory pressure shares the same APFS pool).
- **Rule:** a large-repository run's preconditions are headroom AND load: record both in every `.meta`, refuse to start a 30 min run above ~15 load, and never attribute a floor kill to downloads without the load line.

## 2026-10-09: a sorted scan with a segment-aware predicate breaks on the string boundary
- **Mistake:** workspace member discovery scanned sorted package roots with `take_while(is_under)`, asserting "roots strictly under follow it in sorted order". A sibling (`mods-old`) sorts between the root and its members (`-` below `/`), so the scan stopped early and members fell back to unpinned jobs (evaluator pass 1, Z1).
- **Rule:** when a scan over sorted paths uses a segment-aware predicate (`is_under`, "strictly under"), the `take_while` boundary must be the *string* predicate (shared prefix ⇒ contiguous run) with the segment check as a per-item filter — never the reverse. Test with a sibling whose post-root character sorts below `/` (`-`, `.`, `+`), plus a growth shape with such siblings.

## 2026-10-10: never `git checkout --` a file with uncommitted work; verify the restore path first
- **Mistake:** during phase-4 mutant runs, `git checkout -- crates/rb-frameworks/src/clients.rs` (meant to revert a mutant) discarded the whole phase's uncommitted implementation. Restored from a `/tmp` backup, verified by the test suite — but the backup's freshness had to be established after the fact.
- **Rule:** revert mutants by re-applying the saved bytes (`cp` back the pre-mutant copy), never by `git checkout --`. Before any destructive command, confirm the restore source exists and is current (`diff` it against the tree if unsure). Keep one clearly-named pre-mutant copy per file under `/tmp` for the duration of the mutation runs.

## 2026-10-10: Go's `httptest` lives at `net/http/httptest`, and a wrong import path fails silent
- **Mistake (caught by tests):** the S4 sink walk looked up `net/httptest`; Go's package is `net/http/httptest`, so no recorder chain was ever recognised and three tests came back empty. The import gate returned `None` exactly as designed for a missing import — the design worked, the constant was wrong.
- **Rule:** import-path constants get a positive unit each (a probe asserting the alias resolves), and a failing sink test is debugged by printing the gate's inputs (the resolved import names, the recognised chain indices) before doubting the walk.

## 2026-10-10: a shared DB container bind-mounts its own checkout's `var/`
- **Mistake (environment):** this workspace's e2e failed on AGE loads because the running compose database (started from the original checkout) bind-mounts that checkout's `var/age-load`, not this workspace's. Replacing this workspace's (gitignored) `var/age-load` with a symlink to the mounted directory fixed it.
- **Rule:** when e2e fails on loader paths with a shared database, inspect the container's mounts (`docker inspect … .Mounts`) and point the workspace's gitignored load dir at the mounted source. Never restart or move the shared database for a workspace-local problem.

## 2026-10-10: axios verbs take data before config — a config in arg 1 is data
- **Mistake (caught by e2e):** the S5 fixture wrote `axios.post(url, { ...extOpts })`, putting the spread config in the data position (`post`/`put`/`patch` take `(url, data, config)`); the site stitched exactly like the config-less control, and the stitch unit drafted the same way failed first for the same reason. Only `get`/`delete`/`head`/`options` take `(url, config)`.
- **Rule:** when a fixture or unit exercises a request config, match the verb's arity first (`get(url, config)` for two-arg configs, `post(url, data, config)` for three); assert the pack-level `base=` rendering in the unit before trusting the stitch-level bucket. A config-shaped second argument to a three-arg verb is data, silently.

## 2026-10-10: per-site folder rebuilds fail growth ratios — amortize per file
- **Mistake (caught by the growth test):** the S6 member proof built its per-file index inside a fold that production runs per site: 25→100 members measured 15×. The scopes cache had the same latent shape (a fresh `Folder` per fold); no existing growth test exercised it because none folded densely.
- **Rule:** per-file work (lookup indexes, proof tables, clean-fold memo) belongs in one `Folder` shared across a file's folds, with a fresh step budget per fold so budget semantics are unchanged. Growth-test the shared shape production runs; a per-call folder in the test measures construction, not the walk.

## 2026-10-10: insert-only edits anchored on single unique lines
- **Mistake (repeated):** several `edit` calls spanned existing code that the replacement then dropped (a test body, a function signature line, a loop header) — each caught by `git diff`/build, each repaired by hand. Multi-line `oldString` is most dangerous at its edges.
- **Rule:** prefer pure insertions (`oldString` = one unique anchor line, `newString` = anchor + new content). When replacing, re-read the region first and keep `oldString` minimal; `git diff --stat` after every batch of edits, before running anything. Corroborated 2026-10-10 (S8): in multi-edit batches against one file, single edits silently failed to land despite reporting success (a missing `let lifted`, a missing coverage insert, a missing helper field) — each caught only by grepping for the new text. Never trust a batch result; verify every hunk individually (`grep` the new symbol, `git diff` the file) before building.

## 2026-10-10: a growth test on hitting input measures a constant
- **Mistake (caught by a surviving mutant):** the S8 single-literal growth test used hitting input, so the scan early-returned after the first line at both sizes and a scan-to-EOF quadratic mutant survived (ratio ~1×). The first quadratic mutant (quadratic in nesting, not input size) survived for the same reason the test was blind: it scaled the wrong dimension.
- **Rule:** growth inputs must force the full walk (no-hit input for early-exit scans), and the mutant must be quadratic in the scaled dimension (input size, not depth). Record both numbers: correct code within the bound, the quadratic mutant past it (S8: mutant 693,100 → 11,503,300 vs the 5× bound).

## 2026-10-10: e2e materialises only git-tracked fixture files, and dump goldens live in structure_goldens
- **Mistake (caught by a too-fast pass):** the S9 fixture file was untracked, so `fixture_repo` (which copies `git ls-files`) silently excluded it and `polyglot_monorepo` passed in 3s testing nothing new. Separately, `polyglot_monorepo` covers only guards/determinism — `dump.jsonl`, `status-*`, cards and finds are compared by `polyglot_monorepo_structure_goldens`.
- **Rule:** `git add` fixture files before their first e2e run (staging suffices for `ls-files`); after guards go green, run the structure/scip/diff/path/caller tests for the golden churn, classify every hunk before `RB_BLESS=1`, and re-run the full suite green. A passing e2e faster than the fixture's known index time deserves suspicion, not celebration.

## 2026-10-10: cargo swallows eprintln — run test binaries directly for probes
- **Mistake (repeated):** `cargo test -- --nocapture` showed no probe output three times; the test binary run directly (`./target/debug/deps/<hash> <filter> --nocapture`) printed immediately.
- **Rule:** debug-print through the test binary's path (find the fresh hash by `ls -t`), not through cargo. Delete probe tests before committing (they also shift unit counts).

## 2026-10-10: evaluator pass 1 must probe configuration surfaces and allow-list edges
- **Finding (fixed):** the S9 battery passed with plain sites through reconfigured calls (`superagent.use`, `got`/`ky` `hooks`) because no probe exercised them; the S8 allow-list missed a corpus-evidenced routing type (`ReactorLoadBalancer`).
- **Rule:** for every new client form, probe its configuration surfaces (plugins, interceptors, hooks, custom transports, by-name/spread options) expecting caps; for every narrowing allow-list, probe its boundary types (same-name foreign types, unresolved imports, sibling types in the same tree) expecting both caps and lifts. Survivors that force new rows are the mechanism working — record them as such.

## 2026-10-10: zsh does not word-split — arrays for argv, never strings
- **Mistake (repeated 3×):** `set -- $spec`, unquoted `$ARGS`, and `echo ===` produced mangled URLs/flags and syntax errors in measurement scripts.
- **Rule:** in zsh pass argv as arrays (`args=(...)`, `"${args[@]}"`) or functions with positional params; never unquoted string expansion, never `echo ===`. Verify each script's first real effect (clone lands, binary runs) before the batch.

## 2026-10-10: SCIP symbol keys quote the basename, not the path
- **Mistake:** the new S4 must-exist guard wrote the nested callee as ``src/`models/user.ts`/helper()``; the real key is ``src/models/`user.ts`/helper()`` (directory + backticked basename). The SCIP test failed on the guard while the edge existed.
- **Rule:** never hand-write a guard key for a nested path: dump the fixture once and copy the key byte-for-byte (or simulate `guard_failures` against the dump before blessing).

## 2026-10-10: a surviving mutant can indict the test, not the code
- **Mistake:** the exports-exact-drop mutant survived because the test's exact-vs-`./*` case also resolves through the longest-match loop — the exact-first loop looked redundant. It is not: `./a` vs the longer pattern `./a*` resolves differently.
- **Rule:** when a mutant survives, check whether the test lacks the discriminating shape before touching the code; add the minimal shape (`./a`-vs-`./a*`) that only the mechanism under test decides.

## 2026-10-10: expose the level, don't mirror the walk
- **Mistake (avoided):** the first S5 sketch mirrored `levels()` order in a second function to answer "did resolution reach on-demand". A second walk drifts from the first the moment either changes.
- **Rule:** when a decision needs to know which branch a search took, return the branch from the search (here `levels()` returns its winning `TypeLevel`). One walk, no drift. The `callable=false` path never touches `members()`, so the extra return is recursion-safe by construction.

## 2026-10-10: fan out only the current call's slice of a shared accumulator
- **Mistake:** the S6 edge fan-out did `std::mem::take(&mut out.injections)` and re-sourced every injection — but `out` accumulates across points, so one point's consumers overwrote all previous points' sources (caught by the test: `NeedsOther → Dep` with `sites: 2`).
- **Rule:** when post-processing an accumulator inside a per-item call, `split_off` the length taken before the call's own pushes and fan out only that suffix.

## 2026-10-10: `fixture_repo` copies `git ls-files` — stage new fixture files first
- **Mistake:** the S6 bless ran with two new fixture files untracked, so the e2e indexed a partial fixture (tracked edit present, new files absent) and blessed wrong goldens; caught by the Integr's card counts (+1 symbol instead of +5).
- **Rule:** `git add` new fixture files before any bless run, and sanity-check the blessed diff's file/symbol counts against the fixture change before classifying.

## 2026-10-10: same-key edges merge — isolate negative cases in their own method
- **Mistake:** the S7 allow-list-drop mutant survived because the non-listed `customAudit` call shared its method with the listed `save` call: both edges have the same (source, kind, target, attrs) and merge in the builder, so the extra edge was invisible.
- **Rule:** when a test pins the absence of an edge, give the negative case a source no positive case shares (its own method). Merged evidence is invisible to edge-list asserts.

## 2026-10-10: Tier B defers to SCIP on exact files — so must every heuristic producer
- **Mistake:** the S7 producer edged SCIP-documented files and failed the M3b S7 assert (`heuristic code edge in a SCIP-ok project`): `add_edges` skips `exact_files`, but the new producer did not know about the set.
- **Rule:** every heuristic edge producer takes the exact-files set and skips it. The pre-gate bless failure is the kill proof for the gate-drop mutant.

## 2026-10-11: accumulation order decides what `+=` means — pin the primary use
- **Mistake:** the first S8 chain reader accumulated nearest-first, so a root `=` wiped nearer `+=` additions — breaking the primary `+=` pattern (base list at root, extras below). Caught by writing the accumulation test, which failed.
- **Rule:** apply config chains root-first (`=` replaces, `+=` appends, `!` clears) and pin the root-`=`-plus-subdir-`+=` shape explicitly; it is the reason `+=` exists.

## 2026-10-11: a shadowing exact match hides the mechanism a mutant removes
- **Mistake:** the S8 stop-drop mutant survived because the test's nearer `=` shadowed the farther entries anyway — the stop boundary never decided anything.
- **Rule:** when pinning precedence/shielding, use the weakest nearby form (`+=`, not `=`) so only the mechanism under test decides the outcome. Same family as the same-key-merge lesson.
