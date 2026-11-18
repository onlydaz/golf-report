use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum WordOrder {
    HighWordFirst,
    LowWordFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PlcResult {
    Unmeasured,
    Ok,
    Ng,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlcControl {
    pub format_version: i16,
    pub record_size: i16,
    pub buffer_count: i16,
    pub write_idx: i16,
    pub total_count: i32,
    pub last_idx: i16,
    pub heartbeat: i16,
    pub pc_read_count: i32,
    pub pc_heartbeat: i16,
    pub data_ready: i16,
    pub pc_read_done: i16,
    pub reserve: [u16; 7],
}
impl PlcControl {
    // Heartbeat và các giá trị PC có thể thay đổi độc lập với dữ liệu phép đo.
    pub fn same_buffer_state(&self, other: &Self) -> bool {
        self.format_version == other.format_version
            && self.record_size == other.record_size
            && self.buffer_count == other.buffer_count
            && self.total_count == other.total_count
            && self.write_idx == other.write_idx
            && self.last_idx == other.last_idx
            && self.data_ready == other.data_ready
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlcMeasurementAttempt {
    pub number: u8,
    pub angle_degrees: u16,
    pub push_dist: f32,
    pub bend: f32,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlcMeasurementPoint {
    pub result: PlcResult,
    // Số lượt đo, KHÔNG phải retry_count của schema cũ.
    pub attempt_count: i16,
    pub decide_attempt: i16,
    pub reserve: u16,
    pub position: f32,
    pub attempts: [PlcMeasurementAttempt; 4],
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlcStation {
    pub result: PlcResult,
    pub point_count: i16,
    pub ng_point: i16,
    pub reserve: u16,
    pub load: f32,
    pub reserve_after_load: [u16; 2],
    pub points: [PlcMeasurementPoint; 4],
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlcStickRecord {
    pub buffer_index: usize,
    pub version: i16,
    pub status: u16,
    pub valid_unacknowledged: bool,
    pub stick_no: i32,
    pub product_code: String,
    pub unknown_header_14_19: [u16; 6],
    pub overall_result: PlcResult,
    pub reserve_header: u16,
    pub record_seq: i32,
    pub butt: PlcStation,
    pub tip: PlcStation,
    pub reserve_end: Vec<u16>,
    // Giữ đủ 240 WORD, bao gồm padding và vùng chưa biết, để đối chiếu tài liệu.
    pub raw_registers: Vec<u16>,
}
impl PlcStickRecord {
    pub fn validate_metadata(&self, total_count: Option<i64>) -> Result<(), String> {
        let total = total_count.ok_or("AS200_METADATA: missing snapshot TotalCount")?;
        if total < 0
            || total > i64::from(i32::MAX)
            || self.record_seq < 0
            || i64::from(self.record_seq) > total
        {
            return Err("AS200_METADATA: RecordSeq/TotalCount inconsistent".into());
        }
        Ok(()) // Zero and non-one starting counters are not rejected.
    }
    /// Product identity validation only. PLC owns Overall, station and point results.
    pub fn validate_product_code(&self) -> Result<(), String> {
        if self.product_code.trim().is_empty()
            || !self.product_code.bytes().all(|b| (32..=126).contains(&b))
        {
            return Err("ProductCode missing or contains invalid characters".into());
        }
        Ok(())
    }
    pub fn complete(&self) -> bool {
        self.valid_unacknowledged
            && self.overall_result != PlcResult::Unmeasured
            && self.butt.result != PlcResult::Unmeasured
            && self.tip.result != PlcResult::Unmeasured
    }
}
