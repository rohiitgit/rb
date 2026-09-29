-- Diagnostics keep the repo-relative path even when it is not a File node (e.g. a skipped symlink).
ALTER TABLE rb.diagnostic ADD COLUMN path text;
CREATE INDEX diagnostic_snapshot ON rb.diagnostic (snapshot_id);
-- Supports the evidence → edge cascade and per-edge evidence lookups.
CREATE INDEX edge_evidence_edge ON rb.edge_evidence (snapshot_id, src, kind, dst, provenance);
