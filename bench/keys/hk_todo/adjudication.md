# Adjudication of hk_todo (contract M8b, T4)

Append-only. Each entry comes from a blind adjudicator's verdict on a statement
(`var/bench/labels/hk_todo/statements.txt`, `verdicts.md`), replayed by
`bench/adjudicate.py`. `key.txt` is never edited.

## 2026-10-14: first adjudication

68 statements: every fact on which the key and rb disagree, plus about one in five on
which they agree, written alike and in hash order. The adjudicator read only the pinned checkout
and the statements (transcript audited: no other path). Verdicts: 66 TRUE, 2 FALSE.

65 key-held statements (26 single-op) all TRUE, and one `?` endpoint (`GET /swagger/{}`) TRUE at the key's own line; FALSE: `GET /identity` (registered only in `middleware_test.go`, test code) and `POST /sign-up ?` (the route is `/auth/sign-up`, already in the key).

No entries: every verdict agrees with the key, so the effective key equals `key.txt`.
Caveat: the labeller and the adjudicator are the same model family, so an error both share
would pass both.

## 2026-10-15: second adjudication (re-generated statements)

`rb-bench statements` today emits 72 statements (v1 had 68; the
`*any`-as-parameter normalisation postdates v1, so no statement prints a
bare `?`). The v1 `S19 ENDPOINT GET /swagger/{} ?` (rb-only then, judged
TRUE at the key's own line) is now `S34 ENDPOINT GET /swagger/{}
pkg/handler/handler.go:24` with the concrete registration line, still TRUE
and key-held (`key.txt` line 5). New rb-only statements
(`ENDPOINT GET /identity *`, `ENDPOINT POST /sign-up *`) are judged FALSE:
test-only registration and wrong path. Verdicts: 69 TRUE, 3 FALSE. Every
key-held statement TRUE, every rb-only statement FALSE. No entries.
