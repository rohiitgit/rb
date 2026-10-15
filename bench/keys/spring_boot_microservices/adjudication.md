# Adjudication of spring_boot_microservices (contract M8b, T4)

Append-only. Each entry comes from a blind adjudicator's verdict on a statement
(`var/bench/labels/spring_boot_microservices/statements.txt`, `verdicts.md`), replayed by
`bench/adjudicate.py`. `key.txt` is never edited.

## 2026-10-14: first adjudication

30 statements: every fact on which the key and rb disagree, plus about one in five on
which they agree, written alike and in hash order. The adjudicator read only the pinned checkout
and the statements (transcript audited: no other path). Verdicts: 10 TRUE, 20 FALSE.

10 key-held statements (2 single-op) all TRUE; 20 statements the key lacks all FALSE: each names a repository interface line as querying through inherited CRUD (excluded by FORMAT.md) or an op no source states.

No entries: every verdict agrees with the key, so the effective key equals `key.txt`.
Caveat: the labeller and the adjudicator are the same model family, so an error both share
would pass both.

## 2026-10-15: second adjudication (re-generated statements)

`rb-bench statements` today emits 31 statements (v1 had 30; the
`*any`-as-parameter normalisation postdates v1). The one new statement,
`ENDPOINT GET /orders/{} *`, is key-held and judged TRUE
(`verdicts.md` S7, with the other 30 verdicts unchanged in substance).
Verdicts: 11 TRUE, 20 FALSE. Every key-held statement TRUE, every rb-only
statement FALSE. No entries.
