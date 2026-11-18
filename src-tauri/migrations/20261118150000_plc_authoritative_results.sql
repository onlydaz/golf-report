-- Preserve PLC results independently of diagnostic and detail consistency.
-- Forward-only schema change; no historical results or raw receipts are rewritten.
ALTER TABLE measurements DROP CONSTRAINT measurement_unknown_quality;

-- Locate the original multi-column CHECK by its column dependencies, not by
-- PostgreSQL's generated constraint name. Fail closed on an unexpected schema.
DO $$
DECLARE old_check text;
BEGIN
    SELECT c.conname INTO STRICT old_check
    FROM pg_constraint c
    WHERE c.conrelid = 'measurement_points'::regclass AND c.contype = 'c'
      AND c.conkey @> ARRAY[
          (SELECT attnum FROM pg_attribute WHERE attrelid=c.conrelid AND attname='decide_attempt'),
          (SELECT attnum FROM pg_attribute WHERE attrelid=c.conrelid AND attname='attempt_count')
      ]::smallint[];
    EXECUTE format('ALTER TABLE measurement_points DROP CONSTRAINT %I', old_check);
END $$;
ALTER TABLE measurement_points ADD CONSTRAINT measurement_decide_attempt_bounds
    CHECK (decide_attempt BETWEEN 1 AND 4);

-- Invalid scalar details have no fabricated numeric substitute. Their original
-- WORDs and the diagnostic remain available while the station result is kept.
ALTER TABLE measurement_stations ALTER COLUMN point_count DROP NOT NULL,
    ALTER COLUMN load DROP NOT NULL;
