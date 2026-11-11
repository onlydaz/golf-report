//! AS200 v1: reader/decoder và ACK opt-in theo protocol Simulator hoặc AS200.
#![allow(dead_code)] // API độc lập chờ xác nhận PLC thật trước khi tích hợp.

pub mod ack;
pub mod decode;
pub mod heartbeat;
pub mod map;
pub mod model;
pub mod reader;
#[cfg(test)]
mod tests;

pub use model::WordOrder;
use std::{fmt, time::Duration};

#[derive(Debug, Clone)]
pub struct As200Config {
    pub address: String,
    pub unit_id: u8,
    // TODO kỹ thuật viên xác nhận ánh xạ D -> địa chỉ PDU; tài liệu đề xuất 0.
    pub modbus_address_offset: i32,
    // TODO xác nhận thứ tự WORD cho cả DINT32 và REAL32 trên thiết bị.
    pub word_order: WordOrder,
    pub timeout: Duration,
}
// Cố ý không có Default: caller phải chọn địa chỉ, offset và WordOrder rõ ràng.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum As200Error {
    InvalidData(String),
    Transport(String),
    Timeout,
    Unstable,
    SessionUnconfirmed,
    AckDisabled,
}
impl fmt::Display for As200Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidData(detail) => write!(f, "Dữ liệu AS200 không hợp lệ: {detail}"),
            Self::Transport(detail) => write!(f, "Lỗi Modbus TCP AS200: {detail}"),
            Self::Timeout => write!(f, "Hết thời gian kết nối/đọc AS200"),
            Self::Unstable => write!(f, "Buffer AS200 đang thay đổi; bỏ snapshot và thử lại"),
            Self::SessionUnconfirmed => write!(
                f,
                "TotalCount giảm; cần xác nhận phiên/reset PLC trước khi tiếp tục"
            ),
            Self::AckDisabled => write!(f, "ACK đang bị vô hiệu hóa; không gửi FC06/FC16"),
        }
    }
}
impl std::error::Error for As200Error {}

/// Interface chuẩn bị ACK; không cung cấp transport ghi và không tự gọi ACK.
/// TODO: xác nhận thứ tự PC_ReadCount/PC_ReadDone và session với kỹ thuật viên trước khi hiện thực ghi.
pub trait PlcAcknowledgement {
    fn acknowledge(&mut self, total_count: i32) -> Result<(), As200Error>;
}
pub struct DisabledAcknowledgement;
impl PlcAcknowledgement for DisabledAcknowledgement {
    fn acknowledge(&mut self, _total_count: i32) -> Result<(), As200Error> {
        Err(As200Error::AckDisabled)
    }
}
