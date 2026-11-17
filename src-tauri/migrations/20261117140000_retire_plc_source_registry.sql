-- Retire only an EMPTY legacy registry. Never discard enrollment evidence.
-- SQLx runs this migration transactionally. RESTRICT remains the final dependency gate.
SET LOCAL lock_timeout = '5s';
-- Refuse a filtered read under RLS rather than mistake hidden rows for an empty table.
SET LOCAL row_security = off;
DO $$
DECLARE
    registry oid;
    registry_type oid;
    schema_name text := current_schema();
    populated boolean;
BEGIN
    SELECT c.oid, c.reltype INTO registry, registry_type
    FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
    WHERE n.nspname=schema_name AND c.relname='plc_source_registry'
      AND c.relkind='r';
    IF registry IS NULL THEN
        RAISE EXCEPTION 'Registry retirement requires the existing ordinary table; investigate schema/history, do not reset migrations';
    END IF;
    EXECUTE format('LOCK TABLE %I.plc_source_registry IN ACCESS EXCLUSIVE MODE', schema_name);
    EXECUTE format('SELECT EXISTS(SELECT 1 FROM %I.plc_source_registry)', schema_name) INTO populated;
    IF populated THEN
        RAISE EXCEPTION 'Registry contains data: inventory and preserve it before approving retirement; no rows were deleted';
    END IF;
    -- Normal dependencies include external views/FKs; also inspect row-type users.
    IF EXISTS (
        SELECT 1 FROM pg_depend
        WHERE ((refclassid='pg_class'::regclass AND refobjid=registry)
            OR (refclassid='pg_type'::regclass AND refobjid=registry_type))
          AND deptype NOT IN ('a','i')
          -- PostgreSQL also records normal column dependencies for this table's
          -- own CHECK constraints; those are owned by the table being retired.
          AND NOT (classid='pg_constraint'::regclass AND EXISTS (
              SELECT 1 FROM pg_constraint c WHERE c.oid=pg_depend.objid AND c.conrelid=registry
          ))
    ) OR EXISTS (SELECT 1 FROM pg_inherits WHERE inhparent=registry OR inhrelid=registry)
      OR EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid=registry AND NOT tgisinternal)
      OR EXISTS (SELECT 1 FROM pg_policy WHERE polrelid=registry)
      OR EXISTS (SELECT 1 FROM pg_publication_rel WHERE prrelid=registry)
    THEN
        RAISE EXCEPTION 'Unexpected registry dependency/policy/trigger/publication: inspect consumers before retirement';
    END IF;
    EXECUTE format('DROP TABLE %I.plc_source_registry RESTRICT', schema_name);
END;
$$;
