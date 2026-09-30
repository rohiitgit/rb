-- SCIP indexes, content-addressed (design §5.2): a project whose inputs and indexer are unchanged
-- reuses its index across snapshots and repositories.
CREATE TABLE rb.scip_index (
  input_hash     text        NOT NULL,           -- rb_scip::project_input_hash
  indexer_digest text        NOT NULL,           -- image reference pinned by digest
  status         text        NOT NULL CHECK (status IN ('ok', 'failed', 'timeout')),
  index          bytea,                          -- index.scip when status = 'ok'
  stderr_tail    text        NOT NULL DEFAULT '',
  elapsed_ms     integer     NOT NULL DEFAULT 0,
  created_at     timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (input_hash, indexer_digest)
);
