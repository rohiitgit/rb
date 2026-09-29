CREATE SCHEMA IF NOT EXISTS audit;

CREATE TABLE audit.users (
  user_id bigint NOT NULL,
  event   text   NOT NULL,
  at      timestamptz NOT NULL DEFAULT now()
);
