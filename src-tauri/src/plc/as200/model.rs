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
    /// Quy tắc điểm/lượt và kết quả theo Excel AS200, dùng chung cho mọi nguồn.
    /// Không kiểm tra ngưỡng số đo, quan hệ StickNo/sequence hoặc các bit status chưa chốt.
    pub fn validate_workflow(&self) -> Result<(), String> {
        if self.product_code.trim().is_empty() || !self.product_code.bytes().all(|b| (32..=126).contains(&b)) {
            return Err("ProductCode missing or contains invalid characters".into());
        }
        for (name, station) in [("TIP", &self.tip), ("BUTT", &self.butt)] {
            let measured = match station.result {
                PlcResult::Ok if station.point_count == 4 && station.ng_point == 0 => 4,
                PlcResult::Ng
                    if (1..=4).contains(&station.ng_point)
                        && station.point_count == station.ng_point =>
                {
                    station.point_count as usize
                }
                PlcResult::Unmeasured if station.point_count == 0 && station.ng_point == 0 => 0,
                _ => {
                    return Err(format!(
                        "AS200_WORKFLOW: {name} PointCount/NGPoint/Result inconsistent"
                    ));
                }
            };
            for (index, point) in station.points.iter().enumerate() {
                let expected = if index >= measured {
                    PlcResult::Unmeasured
                } else if station.result == PlcResult::Ng && index + 1 == measured {
                    PlcResult::Ng
                } else {
                    PlcResult::Ok
                };
                let count = point.attempt_count;
                if point.result != expected
                    || match expected {
                        PlcResult::Unmeasured => {
                            count != 0 || point.decide_attempt != 0 || point.position != 0.0
                        }
                        PlcResult::Ng => count != 4 || point.decide_attempt != 4,
                        PlcResult::Ok => !(1..=4).contains(&count) || point.decide_attempt != count,
                    }
                    || point
                        .attempts
                        .iter()
                        .skip(count.max(0) as usize)
                        .any(|a| a.push_dist != 0.0 || a.bend != 0.0)
                {
                    return Err(format!(
                        "AS200_WORKFLOW: {name} point {} has invalid attempts/result or unused measurements",
                        index + 1
                    ));
                }
            }
        }
        let expected = if self.tip.result == PlcResult::Ng || self.butt.result == PlcResult::Ng {
            PlcResult::Ng
        } else if self.tip.result == PlcResult::Ok && self.butt.result == PlcResult::Ok {
            PlcResult::Ok
        } else {
            PlcResult::Unmeasured
        };
        if self.overall_result != expected {
            return Err("AS200_WORKFLOW: OverallResult inconsistent".into());
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
