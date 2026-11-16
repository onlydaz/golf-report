//! AS200 Published Record: one read region and a durable, explicitly authorized ACK handshake.

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
}
impl fmt::Display for As200Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidData(detail) => write!(f, "Dữ liệu AS200 không hợp lệ: {detail}"),
            Self::Transport(detail) => write!(f, "Lỗi Modbus TCP AS200: {detail}"),
            Self::Timeout => write!(f, "Hết thời gian kết nối/đọc AS200"),
            Self::Unstable => write!(f, "Buffer AS200 đang thay đổi; bỏ snapshot và thử lại"),
        }
    }
}
impl std::error::Error for As200Error {}
