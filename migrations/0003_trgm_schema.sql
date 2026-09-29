-- 0001 ran with ag_catalog first on the search_path, so pg_trgm was installed into AGE's schema.
-- Move it to public (no-op on databases created after the migration runner pinned search_path).
DO $$
BEGIN
  IF (SELECT extnamespace::regnamespace::text FROM pg_extension WHERE extname = 'pg_trgm') <> 'public' THEN
    ALTER EXTENSION pg_trgm SET SCHEMA public;
  END IF;
END
$$;
