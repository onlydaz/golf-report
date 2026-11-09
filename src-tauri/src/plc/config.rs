use std::time::Duration;

// Cấu hình thử nghiệm; giữ source ổn định khi chỉ thay đổi địa chỉ mạng.
#[derive(Clone)]
pub struct PlcConfig {
    pub source: String,
    pub address: String,
    pub unit: u8,
    pub poll: Duration,
    pub timeout: Duration,
}
impl Default for PlcConfig {
    fn default() -> Self {
        Self {
            source: "delta-as200-simulator".into(),
            address: "127.0.0.1:1502".into(),
            unit: 1,
            poll: Duration::from_millis(500),
            timeout: Duration::from_secs(2),
        }
    }
}
