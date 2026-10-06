-- The role ad-hoc Cypher runs as (contract M5 D9, S7): it can read the relational schema and the
-- AGE graphs and nothing else, so a function reachable from Cypher that needs more privilege
-- (`pg_read_file`, which the compose superuser may call) is denied. NOLOGIN: sessions reach it only
-- through `SET LOCAL ROLE rb_reader` inside their read-only transaction. Roles are cluster-wide,
-- so this is idempotent across databases and reruns. Each new graph's schema is granted by the
-- publish transaction; the graphs that exist now are granted here. No table changes.
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'rb_reader') THEN
    CREATE ROLE rb_reader NOLOGIN;
  END IF;
END
$$;
GRANT USAGE ON SCHEMA rb, ag_catalog TO rb_reader;
GRANT SELECT ON ALL TABLES IN SCHEMA rb TO rb_reader;
ALTER DEFAULT PRIVILEGES IN SCHEMA rb GRANT SELECT ON TABLES TO rb_reader;
GRANT SELECT ON ag_catalog.ag_graph, ag_catalog.ag_label TO rb_reader;
DO $$
DECLARE g text;
BEGIN
  FOR g IN SELECT name::text FROM ag_catalog.ag_graph WHERE name::text LIKE 'rb\_%' LOOP
    EXECUTE format('GRANT USAGE ON SCHEMA %I TO rb_reader', g);
    EXECUTE format('GRANT SELECT ON ALL TABLES IN SCHEMA %I TO rb_reader', g);
  END LOOP;
END
$$;
