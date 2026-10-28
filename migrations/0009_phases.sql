-- Phases 2-8 storage: events, decisions, deployments, incidents, features.
-- All tables are repo-scoped, append-only logs queried deterministically.
CREATE TABLE IF NOT EXISTS rb.event (
  id          bigserial PRIMARY KEY,
  repo_id     bigint NOT NULL REFERENCES rb.repo ON DELETE CASCADE,
  kind        text NOT NULL CHECK (kind IN ('commit.created','file.changed','pr.merged','branch.deleted')),
  commit_sha  text NOT NULL,
  path        text,
  detail      jsonb NOT NULL DEFAULT '{}',
  created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS event_repo_commit ON rb.event (repo_id, commit_sha);
CREATE INDEX IF NOT EXISTS event_repo_kind ON rb.event (repo_id, kind);

CREATE TABLE IF NOT EXISTS rb.decision (
  id          bigserial PRIMARY KEY,
  repo_id     bigint NOT NULL REFERENCES rb.repo ON DELETE CASCADE,
  title       text NOT NULL,
  reason      text NOT NULL DEFAULT '',
  decided_on  text NOT NULL DEFAULT '',
  source      text NOT NULL DEFAULT '',
  author      text NOT NULL DEFAULT '',
  created_at  timestamptz NOT NULL DEFAULT now(),
  UNIQUE (repo_id, title)
);
CREATE INDEX IF NOT EXISTS decision_title_trgm ON rb.decision USING gin (title gin_trgm_ops);
CREATE INDEX IF NOT EXISTS decision_reason_trgm ON rb.decision USING gin (reason gin_trgm_ops);

CREATE TABLE IF NOT EXISTS rb.deployment (
  id          bigserial PRIMARY KEY,
  repo_id     bigint NOT NULL REFERENCES rb.repo ON DELETE CASCADE,
  service     text NOT NULL,
  commit_sha  text NOT NULL,
  version     text NOT NULL DEFAULT '',
  status      text NOT NULL DEFAULT 'ok' CHECK (status IN ('ok','failed','rolled_back')),
  created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS deployment_repo_svc ON rb.deployment (repo_id, service);

CREATE TABLE IF NOT EXISTS rb.incident (
  id            bigserial PRIMARY KEY,
  repo_id       bigint NOT NULL REFERENCES rb.repo ON DELETE CASCADE,
  service       text NOT NULL,
  deployment_id bigint REFERENCES rb.deployment ON DELETE SET NULL,
  severity      text NOT NULL DEFAULT 'warn' CHECK (severity IN ('info','warn','critical')),
  summary       text NOT NULL,
  created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS incident_repo_svc ON rb.incident (repo_id, service);

CREATE TABLE IF NOT EXISTS rb.feature (
  id          bigserial PRIMARY KEY,
  repo_id     bigint NOT NULL REFERENCES rb.repo ON DELETE CASCADE,
  name        text NOT NULL,
  description text NOT NULL DEFAULT '',
  created_at  timestamptz NOT NULL DEFAULT now(),
  UNIQUE (repo_id, name)
);
CREATE TABLE IF NOT EXISTS rb.feature_link (
  feature_id  bigint NOT NULL REFERENCES rb.feature ON DELETE CASCADE,
  kind        text NOT NULL CHECK (kind IN ('service','endpoint','table','deployment','metric')),
  target      text NOT NULL,
  PRIMARY KEY (feature_id, kind, target)
);
