//! Địa chỉ D theo tài liệu; offset Modbus chỉ được áp dụng khi tạo request.
pub const CONTROL_BASE: u16 = 25000;
pub const CONTROL_SIZE: usize = 20;
pub const RECORD_BASE: u16 = 25020;
pub const RECORD_SIZE: usize = 240;
pub const BUFFER_COUNT: usize = 2;
// Modbus spec allows 125, but the real AS200 rejects >100 WORD per FC03 (exception 4).
pub const FC03_LIMIT: usize = 100;
pub const FORMAT_VERSION: i16 = 1;

pub mod control {
    pub const FORMAT_VERSION: usize = 0;
    pub const RECORD_SIZE: usize = 1;
    pub const BUFFER_COUNT: usize = 2;
    pub const WRITE_IDX: usize = 3;
    pub const TOTAL_COUNT: usize = 4;
    pub const LAST_IDX: usize = 6;
    pub const HEARTBEAT: usize = 7;
    pub const PC_READ_COUNT: usize = 8;
    pub const PC_HEARTBEAT: usize = 10;
    pub const DATA_READY: usize = 11;
    pub const PC_READ_DONE: usize = 12;
}
pub mod record {
    pub const VERSION: usize = 0;
    pub const STATUS: usize = 1;
    pub const STICK_NO: usize = 2;
    pub const PRODUCT_CODE: usize = 4;
    pub const PRODUCT_WORDS: usize = 10;
    pub const UNKNOWN_HEADER: usize = 14;
    pub const OVERALL_RESULT: usize = 20;
    pub const SEQUENCE: usize = 22;
    pub const BUTT: usize = 24;
    pub const TIP: usize = 120;
    pub const BUTT_POINTS: usize = 32;
    pub const TIP_POINTS: usize = 128;
    pub const POINT_SIZE: usize = 22;
    pub const POINT_COUNT: usize = 4;
    pub const ATTEMPT_COUNT: usize = 4;
}

pub fn record_base(index: usize) -> Option<u16> {
    (index < BUFFER_COUNT).then(|| RECORD_BASE + index as u16 * RECORD_SIZE as u16)
}
pub fn point_offset(base: usize, point_number: usize) -> Option<usize> {
    if !(1..=record::POINT_COUNT).contains(&point_number) {
        return None;
    }
    base.checked_add((point_number - 1) * record::POINT_SIZE)
}
pub fn modbus_address(d_address: u16, offset: i32) -> Option<u16> {
    u16::try_from(i32::from(d_address).checked_add(offset)?).ok()
}
