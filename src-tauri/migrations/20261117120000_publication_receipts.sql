-- Preserve all existing Inbox/measurement rows and migration history.
ALTER TABLE plc_inbox
    ADD COLUMN publication_session text,
    ADD COLUMN publication_count integer,
    ADD COLUMN assigned_sku text,
    ADD COLUMN decode_error text;

-- Old rows retain their original session/status invariant. New raw publications
-- have a separate counter identity even when payload RecordSeq is zero.
DO $$
DECLARE item record;
BEGIN
    FOR item IN SELECT conname FROM pg_constraint
        WHERE conrelid='plc_inbox'::regclass AND contype='c'
        AND pg_get_constraintdef(oid) LIKE '%session_id%'
        AND pg_get_constraintdef(oid) LIKE '%processing_status%'
    LOOP
        EXECUTE format('ALTER TABLE plc_inbox DROP CONSTRAINT %I', item.conname);
    END LOOP;
END; $$;
ALTER TABLE plc_inbox ADD CONSTRAINT inbox_session_status CHECK (
    publication_session IS NOT NULL OR
    ((session_id IS NULL) = (processing_status = 'SESSION_UNCONFIRMED'))
);

ALTER TABLE plc_inbox ADD CONSTRAINT publication_identity CHECK (
    (publication_session IS NULL AND publication_count IS NULL) OR
    (publication_session IS NOT NULL AND length(publication_session) BETWEEN 1 AND 128
        AND publication_count IS NOT NULL AND publication_count >= 0)
);
CREATE UNIQUE INDEX plc_inbox_publication_unique
    ON plc_inbox(source_id, publication_session, publication_count)
    WHERE publication_session IS NOT NULL;
CREATE INDEX plc_inbox_projection_queue ON plc_inbox(source_id, received_at, id)
    WHERE measurement_id IS NULL AND decode_error IS NULL;

-- One outstanding receipt; one durable marker BEFORE the whole Count/Done attempt.
-- No per-write phases. An interrupted attempt becomes observation-only.
CREATE TABLE plc_acquisition (
    source_id bigint PRIMARY KEY REFERENCES plc_sources(id),
    session_id text NOT NULL CHECK (length(session_id) BETWEEN 1 AND 128),
    binding text NOT NULL,
    accepted integer NOT NULL DEFAULT 0 CHECK (accepted >= 0),
    last_total integer NOT NULL DEFAULT 0 CHECK (last_total >= accepted),
    anchor_inbox_id bigint REFERENCES plc_inbox(id),
    pending_inbox_id bigint REFERENCES plc_inbox(id),
    ack_attempted boolean NOT NULL DEFAULT false,
    done_observed boolean NOT NULL DEFAULT false,
    blocked_reason text,
    CHECK (NOT done_observed OR ack_attempted),
    CHECK (pending_inbox_id IS NOT NULL OR (NOT ack_attempted AND NOT done_observed))
);
