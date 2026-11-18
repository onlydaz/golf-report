-- Preserve raw RecordSeq independently of the publication counter.
ALTER TABLE measurements
    ADD COLUMN publication_count INTEGER CHECK (publication_count >= 0),
    ADD COLUMN quality_error TEXT,
    ADD COLUMN sku_origin TEXT NOT NULL DEFAULT 'LEGACY'
        CHECK (sku_origin IN ('LEGACY','QC','PLC','MISSING'));

UPDATE measurements m SET publication_count=i.publication_count,
    sku_origin=CASE WHEN i.assigned_sku IS NOT NULL THEN 'QC' ELSE 'PLC' END
FROM plc_inbox i WHERE i.measurement_id=m.id AND i.source_id=m.source_id
    AND i.publication_session=m.session_id AND i.publication_count IS NOT NULL;

DROP INDEX measurements_confirmed_sequence;
CREATE UNIQUE INDEX measurements_confirmed_sequence
    ON measurements(source_id,session_id,record_seq)
    WHERE session_id IS NOT NULL AND publication_count IS NULL;
CREATE UNIQUE INDEX measurements_publication_identity
    ON measurements(source_id,session_id,publication_count)
    WHERE publication_count IS NOT NULL;
ALTER TABLE measurements ADD CONSTRAINT measurement_publication_context CHECK (
    publication_count IS NULL OR
    (session_id IS NOT NULL AND total_count IS NOT NULL AND total_count=publication_count)
);
ALTER TABLE measurements ADD CONSTRAINT measurement_unknown_quality CHECK (
    quality_error IS NULL OR overall_result='UNMEASURED'
);
