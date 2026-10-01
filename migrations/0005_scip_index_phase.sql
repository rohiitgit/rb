-- The phase a failed SCIP job stopped in (contract M3b amendment 56): `fetch` or `index`. NULL on
-- `ok` rows and on `failed` rows written before this column existed, which are read as misses.
ALTER TABLE rb.scip_index
  ADD COLUMN phase text CHECK (phase IN ('fetch', 'index'));
