-- Repository Brain relational schema (design §8.3). The relational layer is the source of truth;
-- AGE graphs are per-snapshot projections of it.
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE EXTENSION IF NOT EXISTS age;

CREATE SCHEMA rb;

CREATE TABLE rb.repo (
  id               bigserial PRIMARY KEY,
  name             text NOT NULL UNIQUE CHECK (name ~ '^[a-z0-9_]{1,40}$'),  -- also used in AGE graph names
  origin_url       text,
  live_snapshot_id bigint
);

-- One row per index run of a commit. Not unique on (repo, commit, tools): a forced rebuild of the
-- same inputs is a new snapshot, and the determinism tests compare exactly such pairs.
CREATE TABLE rb.snapshot (
  id            bigserial PRIMARY KEY,
  repo_id       bigint NOT NULL REFERENCES rb.repo ON DELETE CASCADE,
  commit_sha    text   NOT NULL,
  parent_id     bigint REFERENCES rb.snapshot ON DELETE SET NULL,
  status        text   NOT NULL CHECK (status IN ('building','live','retired','failed')),
  tool_versions jsonb  NOT NULL,
  coverage      jsonb,
  age_graph     text,
  created_at    timestamptz NOT NULL DEFAULT now()   -- metadata only; never part of graph output
);
CREATE INDEX snapshot_repo ON rb.snapshot (repo_id, id);
CREATE UNIQUE INDEX snapshot_one_live ON rb.snapshot (repo_id) WHERE status = 'live';
ALTER TABLE rb.repo ADD FOREIGN KEY (live_snapshot_id) REFERENCES rb.snapshot ON DELETE SET NULL;

CREATE TABLE rb.node (
  snapshot_id bigint NOT NULL REFERENCES rb.snapshot ON DELETE CASCADE,
  id          bigint NOT NULL,
  kind        text   NOT NULL,
  key         text   NOT NULL,
  name        text   NOT NULL,
  file_id     bigint,
  span        int4[],
  attrs       jsonb  NOT NULL DEFAULT '{}',
  PRIMARY KEY (snapshot_id, id),
  UNIQUE (snapshot_id, kind, key)
);
CREATE INDEX node_name_trgm ON rb.node USING gin (name gin_trgm_ops);
CREATE INDEX node_key_trgm  ON rb.node USING gin (key  gin_trgm_ops);

CREATE TABLE rb.edge (
  snapshot_id bigint   NOT NULL,
  src         bigint   NOT NULL,
  dst         bigint   NOT NULL,
  kind        text     NOT NULL,
  provenance  text     NOT NULL,
  certainty   smallint NOT NULL CHECK (certainty BETWEEN 1 AND 5),  -- 5 exact … 1 ambiguous (§6.3)
  attrs       jsonb    NOT NULL DEFAULT '{}',
  PRIMARY KEY (snapshot_id, src, kind, dst, provenance),
  FOREIGN KEY (snapshot_id, src) REFERENCES rb.node ON DELETE CASCADE,
  FOREIGN KEY (snapshot_id, dst) REFERENCES rb.node ON DELETE CASCADE
);
CREATE INDEX edge_in  ON rb.edge (snapshot_id, dst, kind) INCLUDE (src, certainty);
CREATE INDEX edge_out ON rb.edge (snapshot_id, src, kind) INCLUDE (dst, certainty);

CREATE TABLE rb.edge_evidence (
  snapshot_id bigint NOT NULL,
  src bigint NOT NULL, kind text NOT NULL, dst bigint NOT NULL, provenance text NOT NULL,
  file_id bigint NOT NULL,
  span    int4[] NOT NULL,
  FOREIGN KEY (snapshot_id, src, kind, dst, provenance) REFERENCES rb.edge ON DELETE CASCADE
);

CREATE TABLE rb.file_facts_cache (
  blob_sha   text  NOT NULL,
  extractor  text  NOT NULL,
  facts      bytea NOT NULL,
  PRIMARY KEY (blob_sha, extractor)
);

CREATE TABLE rb.diagnostic (
  snapshot_id bigint NOT NULL REFERENCES rb.snapshot ON DELETE CASCADE,
  kind text NOT NULL, file_id bigint, span int4[], detail jsonb
);
