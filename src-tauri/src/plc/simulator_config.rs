use super::as200::{As200Config, WordOrder};
use crate::models::StoredWordOrder;
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// Engineering approval for the deployed AS200 integration, not for an
/// individual endpoint. This release is intended only for company PLCs that
/// use the approved AS200 register map and ACK semantics.
pub const AS200_ACK_SCOPE_APPROVED: bool = true;
pub const AS200_ACK_SCOPE_ID: &str = "company-as200-register-map-v1";

pub fn ack_scope_allows(config: &SimulatorConfig) -> bool {
    AS200_ACK_SCOPE_APPROVED
        && config.mode == "modbus"
        && config.ack_protocol.as_deref() == Some("as200")
        && config.word_order == StoredWordOrder::Low
        && config.modbus_address_offset == 0
        && !config.pc_heartbeat_enabled
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionTransition {
    pub previous_session_id: Option<String>,
    pub previous_total_count: Option<i32>,
    pub previous_ack_count: i32,
    pub expected_total_count: i32,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SimulatorConfig {
    pub technical_setup_confirmed: bool,
    pub plc_heartbeat_timeout_ms: Option<u64>,
    pub pc_heartbeat_enabled: bool,
    pub allow_plc_heartbeat_write: bool,
    pub session_transition: Option<SessionTransition>,
    pub retry_max_attempts: u32,
    pub retry_max_delay_ms: u64,
    pub enabled: bool,
    pub mode: String,
    pub allow_plc_connection: bool,
    pub ack_enabled: bool,
    pub ack_protocol: Option<String>,
    pub address: String,
    pub unit_id: u8,
    pub source_key: String,
    pub confirmed_session_id: Option<String>,
    pub word_order: StoredWordOrder,
    pub modbus_address_offset: i32,
    pub timeout_ms: u64,
    pub poll_interval_ms: u64,
    pub retry_interval_ms: u64,
    pub max_queue: usize,
    pub journal_path: Option<PathBuf>,
}
impl Default for SimulatorConfig {
    fn default() -> Self {
        Self {
            technical_setup_confirmed: false,
            plc_heartbeat_timeout_ms: None,
            pc_heartbeat_enabled: false,
            allow_plc_heartbeat_write: false,
            session_transition: None,
            retry_max_attempts: 8,
            retry_max_delay_ms: 60000,
            enabled: false,
            mode: "simulator".into(),
            allow_plc_connection: false,
            ack_enabled: false,
            ack_protocol: None,
            address: "127.0.0.1:1503".into(),
            unit_id: 1,
            source_key: String::new(),
            confirmed_session_id: None,
            word_order: StoredWordOrder::Low,
            modbus_address_offset: 0,
            timeout_ms: 2000,
            poll_interval_ms: 500,
            retry_interval_ms: 2000,
            max_queue: 128,
            journal_path: None,
        }
    }
}
impl SimulatorConfig {
    /// Resolve runtime state relative to its owning configuration, including legacy defaults.
    pub(super) fn journal_file(&self, configuration: &Path) -> PathBuf {
        let journal = self.journal_path.clone()
            .unwrap_or_else(|| PathBuf::from("as200_capture_journal.json"));
        if journal.is_absolute() {
            journal
        } else {
            configuration.parent().unwrap_or_else(|| Path::new(".")).join(journal)
        }
    }

    /// Transport/read-only checks do not grant collection or write capabilities.
    pub fn validate_read(&self) -> Result<(), String> {
        let address: std::net::SocketAddr = self
            .address
            .parse()
            .map_err(|_| "Address cần IP:port cụ thể")?;
        if address.port() == 0
            || !(1..=247).contains(&self.unit_id)
            || !(100..=30000).contains(&self.timeout_ms)
            || !matches!(self.mode.as_str(), "simulator" | "plc" | "modbus")
            || super::as200::map::modbus_address(25000, self.modbus_address_offset).is_none()
            || super::as200::map::modbus_address(25499, self.modbus_address_offset).is_none()
        {
            return Err("Thông số kết nối đọc không hợp lệ".into());
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), String> {
        self.validate_read()?;
        if self.mode == "modbus" && !self.technical_setup_confirmed {
            return Err("Chưa xác nhận thiết lập kỹ thuật; chưa cho phép thu dữ liệu".into());
        }
        if self
            .plc_heartbeat_timeout_ms
            .is_some_and(|v| !(2000..=3600000).contains(&v))
            || !(1..=100).contains(&self.retry_max_attempts)
            || self.retry_max_delay_ms < self.retry_interval_ms
            || self.retry_max_delay_ms > 3600000
        {
            return Err("Heartbeat/retry policy ngoài phạm vi cấu hình".into());
        }
        if matches!(self.mode.as_str(), "plc" | "modbus")
            && self.pc_heartbeat_enabled
            && (!self.allow_plc_heartbeat_write || !self.allow_plc_connection)
        {
            return Err("Chưa cho phép ghi PC_Heartbeat trên PLC thật".into());
        }
        if let Some(change) = &self.session_transition {
            if self.confirmed_session_id.is_none()
                || change.previous_session_id == self.confirmed_session_id
                || change.reason.trim().is_empty()
                || !(0..=2).contains(&change.expected_total_count)
                || change.previous_ack_count < 0
            {
                return Err(
                    "Đối chiếu phiên cần phiên mới, lý do và bộ đếm cụ thể (0..2 record đang chờ)"
                        .into(),
                );
            }
        }
        if self.ack_enabled
            && !match self.mode.as_str() {
                "plc" | "modbus" => {
                    self.allow_plc_connection && self.ack_protocol.as_deref() == Some("as200")
                }
                "simulator" => {
                    matches!(self.ack_protocol.as_deref(), Some("simulator-v1" | "as200"))
                }
                _ => false,
            }
        {
            return Err(
                "ACK yêu cầu protocol phù hợp: plc=as200; simulator=as200 hoặc simulator-v1".into(),
            );
        }
        if self.ack_enabled && self.confirmed_session_id.is_none() {
            return Err("ACK yêu cầu phiên được xác nhận rõ ràng".into());
        }
        if self.mode == "plc" && !self.allow_plc_connection {
            return Err("Chưa cho phép kết nối PLC thật; cần xác nhận thông số và đặt allow_plc_connection=true rõ ràng".into());
        }
        let address: std::net::SocketAddr = self
            .address
            .parse()
            .map_err(|_| "Address cần IP:port cụ thể")?;
        if self.mode == "simulator"
            && (self.ack_enabled || self.pc_heartbeat_enabled)
            && !address.ip().is_loopback()
        {
            return Err("Quyền ghi ở chế độ Simulator chỉ cho phép loopback".into());
        }
        if self.source_key.trim().is_empty() || self.source_key.chars().count() > 100 {
            return Err("SourceKey ổn định cần 1–100 ký tự".into());
        }
        if self
            .confirmed_session_id
            .as_ref()
            .is_some_and(|v| v.trim().is_empty() || v.chars().count() > 128)
        {
            return Err("Session cần 1–128 ký tự hoặc bỏ trống trường cấu hình".into());
        }
        if !(50..=60000).contains(&self.poll_interval_ms)
            || !(100..=60000).contains(&self.retry_interval_ms)
            || !(2..=1024).contains(&self.max_queue)
        {
            return Err("Poll/retry/max_queue ngoài phạm vi cấu hình".into());
        }
        Ok(())
    }
    pub fn reader(&self) -> As200Config {
        As200Config {
            address: self.address.clone(),
            unit_id: self.unit_id,
            modbus_address_offset: self.modbus_address_offset,
            word_order: match self.word_order {
                StoredWordOrder::Low => WordOrder::LowWordFirst,
                StoredWordOrder::High => WordOrder::HighWordFirst,
            },
            timeout: Duration::from_millis(self.timeout_ms),
        }
    }
    pub fn heartbeat_timeout(&self) -> Option<Duration> {
        // 5 giây chỉ là policy fixture; không kế thừa ngưỡng này sang PLC thật.
        self.plc_heartbeat_timeout_ms
            .or((self.mode == "simulator").then_some(5000))
            .map(Duration::from_millis)
    }
}
pub(super) fn locate(app_directory: Option<&Path>) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = app_directory {
        candidates.push(dir.join("as200_config.toml"));
    }
    // Release uses the caller-provided ProgramData directory; development can use local files.
    if cfg!(debug_assertions) && let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("as200_config.toml"));
        candidates.push(cwd.join("src-tauri/as200_config.toml"));
        // Cargo/Tauri thường khởi chạy backend với cwd là src-tauri.
        if cwd.file_name().is_some_and(|name| name == "src-tauri") {
            if let Some(root) = cwd.parent() {
                candidates.push(root.join("as200_config.toml"));
            }
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}
pub fn load(app_directory: Option<&Path>) -> Result<Option<(SimulatorConfig, PathBuf)>, String> {
    let Some(path) = locate(app_directory) else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(&path).map_err(|e| format!("Đọc cấu hình AS200: {e}"))?;
    let config = parse(&text)?;
    let journal = config.journal_file(&path);
    Ok(Some((config, journal)))
}

/// Read historical configuration without granting any former DEMO reset capability.
/// All other unknown fields remain rejected when deserializing SimulatorConfig.
pub(super) fn configuration_values(text: &str) -> Result<toml::Value, String> {
    let mut values: toml::Value = toml::from_str(text)
        .map_err(|e| format!("Cấu hình AS200 không hợp lệ: {e}"))?;
    if let Some(table) = values.as_table_mut() {
        table.remove("demo_device_id");
        table.remove("demo_boundary_id");
    }
    Ok(values)
}

pub(super) fn parse(text: &str) -> Result<SimulatorConfig, String> {
    let values = configuration_values(text)?;
    let config: SimulatorConfig =
        values.clone().try_into().map_err(|e| format!("Cấu hình AS200 không hợp lệ: {e}"))?;
    if config.mode == "plc" {
        for key in [
            "address",
            "unit_id",
            "word_order",
            "modbus_address_offset",
            "source_key",
            "allow_plc_connection",
        ] {
            if values.get(key).is_none() {
                return Err(format!(
                    "Chế độ PLC yêu cầu cấu hình rõ {key}; không lấy mặc định Simulator"
                ));
            }
        }
    }
    if config.mode == "modbus" {
        for key in ["address", "unit_id"] {
            if values.get(key).is_none() {
                return Err(format!("Thiếu thông số kết nối: {key}"));
            }
        }
        if config.enabled || config.technical_setup_confirmed {
            for key in [
                "technical_setup_confirmed",
                "word_order",
                "modbus_address_offset",
                "source_key",
                "confirmed_session_id",
                "plc_heartbeat_timeout_ms",
            ] {
                if values.get(key).is_none() {
                    return Err(format!("Chưa xác nhận thông số kỹ thuật: {key}"));
                }
            }
            if config.confirmed_session_id.is_none() || config.plc_heartbeat_timeout_ms.is_none() {
                return Err("Thiếu phiên hoặc ngưỡng giám sát đã xác nhận".into());
            }
            config.validate()?;
        }
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unified_profile_requires_explicit_technical_setup_and_independent_write_permissions() {
        let text = "mode='modbus'\nenabled=true\ntechnical_setup_confirmed=true\naddress='192.0.2.10:502'\nunit_id=7\nword_order='HIGH'\nmodbus_address_offset=-1\nsource_key='commissioned-source'\nconfirmed_session_id='confirmed-session'\nplc_heartbeat_timeout_ms=5000\n";
        let mut cfg = parse(text).unwrap();
        assert!(cfg.validate().is_ok());
        assert!(!cfg.ack_enabled);
        assert!(!cfg.pc_heartbeat_enabled);
        cfg.ack_enabled = true;
        cfg.ack_protocol = Some("as200".into());
        assert!(cfg.validate().is_err());
        cfg.allow_plc_connection = true;
        assert!(cfg.validate().is_ok());
        cfg.ack_protocol = Some("simulator-v1".into());
        assert!(cfg.validate().is_err());
        cfg.ack_protocol = Some("as200".into());
        cfg.pc_heartbeat_enabled = true;
        assert!(cfg.validate().is_err());
        cfg.allow_plc_heartbeat_write = true;
        assert!(cfg.validate().is_ok());
        use crate::plc::as200::ack::{AckWriter, HeartbeatWriter};
        assert!(AckWriter::new(cfg.reader(), "modbus", true, Some("as200"), false).is_err());
        assert!(AckWriter::new(cfg.reader(), "modbus", true, Some("as200"), true).is_ok());
        assert!(HeartbeatWriter::new(cfg.reader(), "modbus", true, true, false).is_err());
        assert!(HeartbeatWriter::new(cfg.reader(), "modbus", true, true, true).is_ok());
        assert!(parse(&text.replace("word_order='HIGH'\n", "")).is_err());
        assert!(
            parse(&text.replace(
                "technical_setup_confirmed=true",
                "technical_setup_confirmed=false"
            ))
            .is_err()
        );
    }
    #[test]
    fn heartbeat_policy_does_not_inherit_simulator_threshold_or_ack_permission() {
        let mut cfg = SimulatorConfig {
            source_key: "policy".into(),
            mode: "plc".into(),
            allow_plc_connection: true,
            ..Default::default()
        };
        assert!(cfg.heartbeat_timeout().is_none());
        assert!(!cfg.pc_heartbeat_enabled);
        assert!(!cfg.allow_plc_heartbeat_write);
        assert!(!cfg.ack_enabled);
        cfg.pc_heartbeat_enabled = true;
        assert!(cfg.validate().is_err());
        cfg.allow_plc_heartbeat_write = true;
        assert!(cfg.validate().is_ok());
        assert!(!cfg.ack_enabled);
        cfg.mode = "simulator".into();
        assert_eq!(cfg.heartbeat_timeout(), Some(Duration::from_secs(5)));
        cfg.plc_heartbeat_timeout_ms = Some(2500);
        assert_eq!(cfg.heartbeat_timeout(), Some(Duration::from_millis(2500)));
    }
    #[test]
    fn plc_mode_never_inherits_simulator_connection_parameters() {
        assert!(parse("mode='plc'\nallow_plc_connection=true").is_err());
        let text = "mode='plc'\naddress='127.0.0.1:1503'\nunit_id=1\nword_order='LOW'\nmodbus_address_offset=0\nsource_key='explicit-test'\nallow_plc_connection=true";
        let mut cfg = parse(text).unwrap();
        assert!(cfg.validate().is_ok());
        cfg.allow_plc_connection = false;
        assert!(cfg.validate().is_err());
        cfg.allow_plc_connection = true;
        cfg.ack_enabled = true;
        assert!(cfg.validate().is_err());
        cfg.mode = "simulator".into();
        assert!(cfg.validate().is_err());
        cfg.mode = "plc".into();
        cfg.ack_protocol = Some("as200".into());
        cfg.confirmed_session_id = Some("explicit-test-session".into());
        assert!(cfg.validate().is_ok());
        cfg.allow_plc_connection = false;
        assert!(cfg.validate().is_err());
    }
}
