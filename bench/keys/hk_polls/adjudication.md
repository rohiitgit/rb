# Adjudication of hk_polls (contract M8b, T4)

Append-only. Each entry comes from a blind adjudicator's verdict on a statement
(`var/bench/labels/hk_polls/statements.txt`, `verdicts.md`), replayed by
`bench/adjudicate.py`. `key.txt` is never edited.

## 2026-10-14: first adjudication

35 statements: every fact on which the key and rb disagree, plus about one in five on
which they agree, written alike and in hash order. The adjudicator read only the pinned checkout
and the statements (transcript audited: no other path). Verdicts: 15 TRUE, 20 FALSE.

15 key-held statements (6 single-op) all TRUE, including the labeller's uncertain `findById` redeclaration (PollRepository.java:16) and the V2 migration insert; 20 statements the key lacks all FALSE (repository interface lines, inherited CRUD, ops no source states).

No entries: every verdict agrees with the key, so the effective key equals `key.txt`.
Caveat: the labeller and the adjudicator are the same model family, so an error both share
would pass both.

## 2026-10-15: second adjudication (re-generated statements)

`rb-bench statements` today emits 37 statements (v1 had 35). The two new
statements are key-held and judged TRUE: `ENDPOINT GET
/api/users/{}/votes *` (`verdicts.md` S15, `UserController.java:78`) and
`ENDPOINT GET /api/user/me *` (S34, `UserController.java:39`).
Verdicts: 17 TRUE, 20 FALSE. Every key-held statement TRUE, every rb-only
statement FALSE. No entries.
