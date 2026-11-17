-- USER REVIEW ONLY: not executed by the agent.
-- This script targets ONLY the 14 sources created by run grit-1794928744834.
-- Default ROLLBACK rehearses cleanup. Review counts before changing it to COMMIT.
-- Stop other clients using these test sources before applying cleanup.
BEGIN;
SET LOCAL search_path = public;
SET LOCAL lock_timeout = '5s';
SET LOCAL statement_timeout = '30s';
DO $$ BEGIN
  IF current_database() <> 'golf_report' THEN
    RAISE EXCEPTION 'Wrong database';
  END IF;
END $$;

CREATE TEMP TABLE integration_cleanup_sources (id bigint PRIMARY KEY, source_key text NOT NULL) ON COMMIT DROP;
INSERT INTO integration_cleanup_sources VALUES
 (2,'grit-1794928744834-worker-0'),
 (6,'grit-1794928744834-worker-1'),
 (7,'grit-1794928744834-worker-2'),
 (8,'grit-1794928744834-worker-3'),
 (9,'grit-1794928744834-worker-4'),
 (10,'grit-1794928744834-legacy-5'),
 (11,'grit-1794928744834-worker-6'),
 (14,'grit-1794928744834-worker-7'),
 (16,'grit-1794928744834-worker-8'),
 (19,'grit-1794928744834-worker-9'),
 (21,'grit-1794928744834-worker-10'),
 (23,'grit-1794928744834-worker-11'),
 (25,'grit-1794928744834-report-12'),
 (26,'grit-1794928744834-services-13');

SELECT s.id, s.source_key FROM public.plc_sources s
JOIN integration_cleanup_sources t ON t.id=s.id AND t.source_key=s.source_key
ORDER BY s.id FOR UPDATE OF s;
DO $$ BEGIN
  IF (SELECT count(*) FROM public.plc_sources s JOIN integration_cleanup_sources t
      ON t.id=s.id AND t.source_key=s.source_key) <> 14 THEN
    RAISE EXCEPTION 'Source manifest differs; review manually, do not broaden scope';
  END IF;
  IF EXISTS (SELECT 1 FROM public.plc_acquisition a
      WHERE a.source_id NOT IN (SELECT id FROM integration_cleanup_sources)
      AND (a.anchor_inbox_id IN (SELECT id FROM public.plc_inbox WHERE source_id IN (SELECT id FROM integration_cleanup_sources))
        OR a.pending_inbox_id IN (SELECT id FROM public.plc_inbox WHERE source_id IN (SELECT id FROM integration_cleanup_sources))))
  THEN RAISE EXCEPTION 'External ACK state references test data'; END IF;
END $$;

-- Expected before cleanup: acquisition=11, inbox=38, measurements=25.
SELECT 'plc_acquisition' AS object, count(*) FROM public.plc_acquisition WHERE source_id IN (SELECT id FROM integration_cleanup_sources)
UNION ALL SELECT 'plc_inbox',count(*) FROM public.plc_inbox WHERE source_id IN (SELECT id FROM integration_cleanup_sources)
UNION ALL SELECT 'measurements',count(*) FROM public.measurements WHERE source_id IN (SELECT id FROM integration_cleanup_sources);

DELETE FROM public.plc_acquisition WHERE source_id IN (SELECT id FROM integration_cleanup_sources);
DELETE FROM public.plc_inbox WHERE source_id IN (SELECT id FROM integration_cleanup_sources);
DELETE FROM public.measurement_attempts a USING public.measurement_points p, public.measurement_stations st, public.measurements m
 WHERE a.point_id=p.id AND p.station_id=st.id AND st.measurement_id=m.id
 AND m.source_id IN (SELECT id FROM integration_cleanup_sources);
DELETE FROM public.measurement_points p USING public.measurement_stations st, public.measurements m
 WHERE p.station_id=st.id AND st.measurement_id=m.id AND m.source_id IN (SELECT id FROM integration_cleanup_sources);
DELETE FROM public.measurement_stations st USING public.measurements m
 WHERE st.measurement_id=m.id AND m.source_id IN (SELECT id FROM integration_cleanup_sources);
DELETE FROM public.measurements WHERE source_id IN (SELECT id FROM integration_cleanup_sources);
-- Compatible with both the old schema and the empty-registry retirement migration.
DO $$ BEGIN
  IF to_regclass('public.plc_source_registry') IS NOT NULL THEN
    EXECUTE 'DELETE FROM public.plc_source_registry WHERE source_id IN (SELECT id FROM integration_cleanup_sources)';
  END IF;
END $$;
DELETE FROM public.plc_sources s USING integration_cleanup_sources t WHERE s.id=t.id AND s.source_key=t.source_key;

-- No migration history, production source, sequence or configuration is reset.
ROLLBACK;
