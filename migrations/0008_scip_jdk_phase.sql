-- A Maven job that failed with `release version N not supported` (N above 21) and was
-- re-run on the JDK 25 image stores `status = 'ok', phase = 'jdk'` (contract Part B):
-- the index is the JDK 25 run's, with the JDK 21 run's tail. No existing row can hold
-- the new value (no binary has written it), so no row changes meaning.
ALTER TABLE rb.scip_index DROP CONSTRAINT scip_index_phase_check;
ALTER TABLE rb.scip_index ADD CONSTRAINT scip_index_phase_check CHECK (phase IN ('fetch', 'index', 'jdk'));
