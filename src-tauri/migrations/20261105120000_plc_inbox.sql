-- Không thay đổi các phép đo cũ. Inbox giữ dữ liệu để retry sau khi khởi động lại.
CREATE TABLE plc_sources (
    source TEXT PRIMARY KEY,
    last_sequence INTEGER NOT NULL,
    generation BIGINT NOT NULL DEFAULT 0
);
CREATE TABLE plc_inbox (
    source TEXT NOT NULL REFERENCES plc_sources(source),
    generation BIGINT NOT NULL,
    sequence_id INTEGER NOT NULL,
    payload JSONB NOT NULL,
    measurement_id BIGINT REFERENCES measurements(id),
    received_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (source, generation, sequence_id)
);
