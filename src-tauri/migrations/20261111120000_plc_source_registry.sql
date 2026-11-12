-- Technical enrollment is separate from a successful FC03 probe. This table
-- records an externally verified physical identity; it does not grant an ACK
-- lease or prove that another laptop has stopped writing to the PLC.
CREATE TABLE plc_source_registry (
    source_id BIGINT PRIMARY KEY REFERENCES plc_sources(id) ON DELETE RESTRICT,
    physical_identity TEXT NOT NULL UNIQUE CHECK (length(btrim(physical_identity)) > 0),
    endpoint_ip INET NOT NULL,
    endpoint_port INTEGER NOT NULL CHECK (endpoint_port BETWEEN 1 AND 65535),
    unit_id INTEGER NOT NULL CHECK (unit_id BETWEEN 1 AND 247),
    session_id TEXT NOT NULL CHECK (length(btrim(session_id)) > 0),
    as200_verified BOOLEAN NOT NULL DEFAULT FALSE,
    ack_write_approved BOOLEAN NOT NULL DEFAULT FALSE,
    verified_at TIMESTAMPTZ,
    CHECK (NOT ack_write_approved OR as200_verified),
    UNIQUE (endpoint_ip, endpoint_port, unit_id)
);
