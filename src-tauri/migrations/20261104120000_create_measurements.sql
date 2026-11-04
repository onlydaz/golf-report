CREATE TABLE IF NOT EXISTS measurements (
    id BIGSERIAL PRIMARY KEY,
    product_code VARCHAR(50) NOT NULL,
    club_number VARCHAR(50) NOT NULL,
    measured_at TIMESTAMPTZ NOT NULL,
    weight NUMERIC(10, 3) NOT NULL CHECK (weight >= 0),
    tip_result VARCHAR(20) NOT NULL CHECK (tip_result IN ('OK', 'NG')),
    butt_result VARCHAR(20) NOT NULL CHECK (butt_result IN ('OK', 'NG')),
    butt_distance NUMERIC(10, 3) NOT NULL CHECK (butt_distance >= 0),
    overall_result VARCHAR(20) NOT NULL CHECK (overall_result IN ('OK', 'NG')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS tip_positions (
    id BIGSERIAL PRIMARY KEY,
    measurement_id BIGINT NOT NULL REFERENCES measurements(id) ON DELETE RESTRICT,
    position_number SMALLINT NOT NULL CHECK (position_number BETWEEN 1 AND 4),
    final_distance NUMERIC(10, 3),
    retry_count SMALLINT,
    result VARCHAR(20) NOT NULL CHECK (result IN ('OK', 'NG', 'SKIPPED')),
    UNIQUE (measurement_id, position_number),
    CHECK (retry_count IS NULL OR retry_count BETWEEN 0 AND 4),
    CHECK (
        (result = 'SKIPPED' AND final_distance IS NULL AND retry_count IS NULL)
        OR (result IN ('OK', 'NG') AND final_distance IS NOT NULL AND final_distance >= 0 AND retry_count IS NOT NULL)
    ),
    CHECK (result <> 'NG' OR retry_count = 4)
);

CREATE INDEX IF NOT EXISTS idx_measurements_product_code ON measurements (product_code);
CREATE INDEX IF NOT EXISTS idx_measurements_club_number ON measurements (club_number);
CREATE INDEX IF NOT EXISTS idx_measurements_measured_at ON measurements (measured_at DESC);
CREATE INDEX IF NOT EXISTS idx_measurements_overall_result ON measurements (overall_result);
CREATE INDEX IF NOT EXISTS idx_tip_positions_measurement_id ON tip_positions (measurement_id);
