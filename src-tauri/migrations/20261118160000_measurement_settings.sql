-- Operator-entered visualization setpoints, independent of PLC measurements.
-- No defaults or sample data; never used for PLC results, acquisition or ACK.
CREATE TABLE measurement_settings (
    product_code TEXT NOT NULL CHECK (product_code = btrim(product_code) AND char_length(product_code) BETWEEN 1 AND 50),
    station_type TEXT NOT NULL CHECK (station_type IN ('BUTT','TIP')),
    load_g DOUBLE PRECISION CHECK (load_g >= 0 AND load_g < 'Infinity'::float8),
    push_dist_mm DOUBLE PRECISION CHECK (push_dist_mm >= 0 AND push_dist_mm < 'Infinity'::float8),
    bend_lower_mm DOUBLE PRECISION CHECK (bend_lower_mm > '-Infinity'::float8 AND bend_lower_mm < 'Infinity'::float8),
    bend_upper_mm DOUBLE PRECISION CHECK (bend_upper_mm > '-Infinity'::float8 AND bend_upper_mm < 'Infinity'::float8),
    positions_mm DOUBLE PRECISION[] NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (product_code, station_type),
    CHECK (bend_lower_mm <= bend_upper_mm),
    CHECK (array_ndims(positions_mm)=1 AND array_lower(positions_mm,1)=1
        AND cardinality(positions_mm)=CASE station_type WHEN 'BUTT' THEN 4 ELSE 1 END),
    CHECK ((positions_mm[1] IS NULL OR (positions_mm[1] >= 0 AND positions_mm[1] < 'Infinity'::float8))
       AND (positions_mm[2] IS NULL OR (positions_mm[2] >= 0 AND positions_mm[2] < 'Infinity'::float8))
       AND (positions_mm[3] IS NULL OR (positions_mm[3] >= 0 AND positions_mm[3] < 'Infinity'::float8))
       AND (positions_mm[4] IS NULL OR (positions_mm[4] >= 0 AND positions_mm[4] < 'Infinity'::float8)))
);
