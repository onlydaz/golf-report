-- NULL means the old worker cannot tell whether Done was started.
-- Never backfill an outstanding attempted ACK as definitely unsent.
ALTER TABLE plc_acquisition ADD COLUMN done_started boolean;
UPDATE plc_acquisition SET done_started=false WHERE NOT ack_attempted;
ALTER TABLE plc_acquisition ALTER COLUMN done_started SET DEFAULT false;
ALTER TABLE plc_acquisition ADD CONSTRAINT done_requires_attempt
    CHECK (done_started IS DISTINCT FROM true OR ack_attempted);
ALTER TABLE plc_acquisition ADD CONSTRAINT observed_done_is_not_unsent
    CHECK (NOT done_observed OR done_started IS DISTINCT FROM false);
