-- Khởi tạo cho database development trống; không chứa DROP/TRUNCATE hay dữ liệu mẫu.
CREATE TABLE plc_sources (
    id BIGSERIAL PRIMARY KEY,
    source_key VARCHAR(100) NOT NULL UNIQUE CHECK (length(btrim(source_key)) > 0),
    description TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE measurements (
    id BIGSERIAL PRIMARY KEY,
    source_id BIGINT NOT NULL REFERENCES plc_sources(id) ON DELETE RESTRICT,
    session_id TEXT CHECK (session_id IS NULL OR length(btrim(session_id)) > 0),
    record_seq INTEGER NOT NULL,
    total_count BIGINT,
    product_code VARCHAR(50) NOT NULL,
    club_number INTEGER NOT NULL,
    -- Thời gian phần mềm nhận bản ghi, không phải timestamp đo từ PLC.
    measured_at TIMESTAMPTZ NOT NULL,
    overall_result VARCHAR(10) NOT NULL CHECK (overall_result IN ('UNMEASURED','OK','NG')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX measurements_confirmed_sequence ON measurements(source_id,session_id,record_seq) WHERE session_id IS NOT NULL;
CREATE INDEX measurements_product_code ON measurements(product_code);
CREATE INDEX measurements_club_number ON measurements(club_number);
CREATE INDEX measurements_measured_at ON measurements(measured_at DESC,id DESC);
CREATE INDEX measurements_overall_result ON measurements(overall_result);
CREATE TABLE measurement_stations (
    id BIGSERIAL PRIMARY KEY,
    measurement_id BIGINT NOT NULL REFERENCES measurements(id) ON DELETE RESTRICT,
    station_type VARCHAR(4) NOT NULL CHECK (station_type IN ('TIP','BUTT')),
    result VARCHAR(10) NOT NULL CHECK (result IN ('UNMEASURED','OK','NG')),
    point_count INTEGER NOT NULL CHECK (point_count BETWEEN 0 AND 4),
    ng_point INTEGER CHECK (ng_point BETWEEN 0 AND 4),
    load NUMERIC NOT NULL CHECK (load::text NOT IN ('NaN','Infinity','-Infinity')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(measurement_id,station_type)
);
CREATE TABLE measurement_points (
    id BIGSERIAL PRIMARY KEY,
    station_id BIGINT NOT NULL REFERENCES measurement_stations(id) ON DELETE RESTRICT,
    point_number INTEGER NOT NULL CHECK (point_number BETWEEN 1 AND 4),
    result VARCHAR(10) NOT NULL CHECK (result IN ('UNMEASURED','OK','NG')),
    attempt_count INTEGER NOT NULL CHECK (attempt_count BETWEEN 0 AND 4),
    decide_attempt INTEGER CHECK (decide_attempt BETWEEN 1 AND 4 AND decide_attempt <= attempt_count),
    position NUMERIC CHECK (position::text NOT IN ('NaN','Infinity','-Infinity')),
    UNIQUE(station_id,point_number)
);
CREATE TABLE measurement_attempts (
    id BIGSERIAL PRIMARY KEY,
    point_id BIGINT NOT NULL REFERENCES measurement_points(id) ON DELETE RESTRICT,
    attempt_number INTEGER NOT NULL CHECK (attempt_number BETWEEN 1 AND 4),
    push_dist NUMERIC CHECK (push_dist::text NOT IN ('NaN','Infinity','-Infinity')),
    bend NUMERIC CHECK (bend::text NOT IN ('NaN','Infinity','-Infinity')),
    UNIQUE(point_id,attempt_number)
);
CREATE FUNCTION as200_valid_registers(words JSONB) RETURNS BOOLEAN
LANGUAGE SQL IMMUTABLE STRICT AS $$
    SELECT CASE WHEN jsonb_typeof(words) <> 'array' THEN false
        WHEN jsonb_array_length(words) <> 240 THEN false
        ELSE NOT EXISTS (
            SELECT 1 FROM jsonb_array_elements(words) AS item(word)
            WHERE NOT CASE WHEN jsonb_typeof(word) = 'number' AND word::text ~ '^[0-9]+$'
                THEN (word::text)::numeric BETWEEN 0 AND 65535 ELSE false END
        ) END
$$;
CREATE TABLE plc_inbox (
    id BIGSERIAL PRIMARY KEY,
    source_id BIGINT NOT NULL REFERENCES plc_sources(id) ON DELETE RESTRICT,
    session_id TEXT CHECK (session_id IS NULL OR length(btrim(session_id)) > 0),
    -- Delivery key do caller giữ nguyên khi retry; KHÔNG phải session PLC giả.
    ingest_key VARCHAR(200) NOT NULL CHECK (length(btrim(ingest_key)) > 0),
    record_seq INTEGER NOT NULL,
    total_count BIGINT,
    buffer_index INTEGER NOT NULL CHECK (buffer_index IN (0,1)),
    raw_registers JSONB NOT NULL CHECK (as200_valid_registers(raw_registers)),
    word_order VARCHAR(4) NOT NULL CHECK (word_order IN ('LOW','HIGH')),
    received_at TIMESTAMPTZ NOT NULL,
    processing_status VARCHAR(20) NOT NULL CHECK (processing_status IN ('PENDING','PROCESSED','FAILED','SESSION_UNCONFIRMED')),
    measurement_id BIGINT UNIQUE REFERENCES measurements(id) ON DELETE RESTRICT,
    error_message TEXT,
    processed_at TIMESTAMPTZ,
    UNIQUE(source_id,ingest_key),
    CHECK ((session_id IS NULL) = (processing_status = 'SESSION_UNCONFIRMED')),
    CHECK ((processing_status = 'PROCESSED') = (measurement_id IS NOT NULL AND processed_at IS NOT NULL)),
    CHECK (processing_status = 'PROCESSED' OR (measurement_id IS NULL AND processed_at IS NULL)),
    CHECK ((processing_status = 'FAILED') = (error_message IS NOT NULL))
);
CREATE UNIQUE INDEX inbox_confirmed_sequence ON plc_inbox(source_id,session_id,record_seq) WHERE session_id IS NOT NULL;
CREATE INDEX inbox_processing_queue ON plc_inbox(processing_status,received_at,id);
CREATE INDEX inbox_source_received ON plc_inbox(source_id,received_at DESC);
