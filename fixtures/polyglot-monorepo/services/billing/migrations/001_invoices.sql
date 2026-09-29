CREATE SCHEMA IF NOT EXISTS billing;

CREATE TABLE billing.invoices (
  id    bigserial PRIMARY KEY,
  total bigint NOT NULL,
  paid  boolean NOT NULL DEFAULT false
);
