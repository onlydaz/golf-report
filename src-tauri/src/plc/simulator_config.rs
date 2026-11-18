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
        && !config.read_only
        && config.control_start == super::as200::map::CONTROL_BASE
        && config.layout_profile == "as200-published-v1"
        && config.buffer_count == super::as200::map::BUFFER_COUNT
        && config.record_size == super::as200::map::RECORD_SIZE
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SimulatorConfig {
    pub layout_profile: String,
    pub control_start: u16,
    pub buffer_count: usize,
    pub record_size: usize,
    pub auto_reconnect: bool,
    pub read_only: bool,
    pub technical_setup_confirmed: bool,
    pub plc_heartbeat_timeout_ms: Option<u64>,
    pub pc_heartbeat_enabled: bool,
    pub allow_plc_heartbeat_write: bool,
    // Parsed only for compatibility with existing TOML; no Journal retry queue.
    pub retry_max_attempts: u32,
    pub retry_max_delay_ms: u64,
    pub enabled: bool,
    pub mode: String,
    pub allow_plc_connection: bool,
    pub ack_enabled: bool,
    pub ack_protocol: Option<String>,
    pub address: String,
    pub plc_local_ip: Option<std::net::IpAddr>,
    pub plc_adapter: Option<super::network::ManualAdapter>,
    pub unit_id: u8,
    pub source_key: String,
    pub confirmed_session_id: Option<String>,
    pub word_order: StoredWordOrder,
    pub modbus_address_offset: i32,
    pub timeout_ms: u64,
    pub poll_interval_ms: u64,
    pub retry_interval_ms: u64,
    // Legacy TOML option; acquisition has one pending PostgreSQL receipt.
    pub max_queue: usize,
    pub journal_path: Option<PathBuf>,
    pub operator_sku: Option<String>,
}
impl Default for SimulatorConfig {
    fn default() -> Self {
        Self {
            layout_profile: "as200-published-v1".into(),
            control_start: super::as200::map::CONTROL_BASE,
            buffer_count: super::as200::map::BUFFER_COUNT,
            record_size: super::as200::map::RECORD_SIZE,
            auto_reconnect: true,
            read_only: false,
            technical_setup_confirmed: false,
            plc_heartbeat_timeout_ms: None,
            pc_heartbeat_enabled: false,
            allow_plc_heartbeat_write: false,
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
            operator_sku: None,
            plc_local_ip: None,
            plc_adapter: None,
        }
    }
}
impl SimulatorConfig {
    /// Diagnostic only: passing configuration checks does not prove receipt/PLC ACK success.
    pub(super) fn ack_config_block_reason(&self) -> Option<&'static str> {
        if !self.ack_enabled {
            Some(
                "ack_enabled=false trong cấu hình đã nạp; sửa TOML không cập nhật Worker đang chạy, cần dừng thu và khởi động lại ứng dụng",
            )
        } else if self.read_only {
            Some("read_only=true không cho phép ghi ACK")
        } else if !self.technical_setup_confirmed {
            Some("technical_setup_confirmed=false; chưa xác nhận thiết lập kỹ thuật")
        } else if !self.allow_plc_connection {
            Some("allow_plc_connection=false; chưa cấp quyền kết nối/ghi")
        } else if self
            .confirmed_session_id
            .as_deref()
            .is_none_or(|s| s.trim().is_empty())
        {
            Some("confirmed_session_id chưa được cấu hình hợp lệ")
        } else if self.pc_heartbeat_enabled || self.allow_plc_heartbeat_write {
            Some("Published Record yêu cầu tắt mọi quyền ghi PC Heartbeat")
        } else if self.ack_protocol.as_deref() != Some("as200") {
            Some("ack_protocol chưa được xác nhận là as200")
        } else if !ack_scope_allows(self) {
            Some(
                "ACK scope không phù hợp: cần mode=modbus, profile as200-published-v1, LOW, offset=0, D25000, 240 WORD và 2 buffer do PLC quản lý",
            )
        } else {
            None
        }
    }

    // Timing, source IP and a safely stopped operator SKU may be saved separately;
    // identity, map and write permissions need an app restart.
    pub(super) fn same_environment(&self, other: &Self) -> bool {
        self.address == other.address
            && self.unit_id == other.unit_id
            && self.mode == other.mode
            && self.source_key == other.source_key
            && self.confirmed_session_id == other.confirmed_session_id
            && self.journal_path == other.journal_path
            && self.layout_profile == other.layout_profile
            && self.control_start == other.control_start
            && self.buffer_count == other.buffer_count
            && self.record_size == other.record_size
            && self.word_order == other.word_order
            && self.modbus_address_offset == other.modbus_address_offset
            && self.read_only == other.read_only
            && self.ack_enabled == other.ack_enabled
            && self.ack_protocol == other.ack_protocol
            && self.allow_plc_connection == other.allow_plc_connection
            && self.technical_setup_confirmed == other.technical_setup_confirmed
            && self.pc_heartbeat_enabled == other.pc_heartbeat_enabled
            && self.allow_plc_heartbeat_write == other.allow_plc_heartbeat_write
    }

    /// Resolve the retained legacy evidence/Worker lock path. Acquisition no
    /// longer writes a JSON Journal; the configured path stays stable for cutover.
    pub(super) fn journal_file(&self, configuration: &Path) -> PathBuf {
        let journal = self
            .journal_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("as200_capture_journal.json"));
        if journal.is_absolute() {
            journal
        } else {
            configuration
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(journal)
        }
    }

    /// Transport/read-only checks do not grant collection or write capabilities.
    pub fn validate_read(&self) -> Result<(), String> {
        self.validate_layout()?;
        let address: std::net::SocketAddr = self
            .address
            .parse()
            .map_err(|_| "Address cần IP:port cụ thể")?;
        // Legacy source IP is parsed for compatibility only; never used for sockets.
        if address.port() == 0
            || !(1..=247).contains(&self.unit_id)
            || !(100..=30000).contains(&self.timeout_ms)
            || !matches!(self.mode.as_str(), "simulator" | "plc" | "modbus")
            || super::as200::map::modbus_address(self.control_start, self.modbus_address_offset)
                .is_none()
            || super::as200::map::modbus_address(self.memory_end()?, self.modbus_address_offset)
                .is_none()
        {
            return Err("Thông số kết nối đọc không hợp lệ".into());
        }
        Ok(())
    }
    pub fn memory_end(&self) -> Result<u16, String> {
        let length = self
            .record_size
            .checked_mul(self.buffer_count)
            .and_then(|v| v.checked_add(super::as200::map::CONTROL_SIZE))
            .ok_or("Dung lượng vùng nhớ PLC vượt phạm vi")?;
        u16::try_from(
            usize::from(self.control_start)
                .checked_add(length.saturating_sub(1))
                .ok_or("Vùng nhớ PLC vượt phạm vi")?,
        )
        .map_err(|_| "Vùng nhớ PLC vượt D65535".into())
    }
    pub fn validate_layout(&self) -> Result<(), String> {
        if self.layout_profile != "as200-published-v1" {
            return Err(
                "Layout PLC chưa được hỗ trợ; chỉ có profile AS200 Published v1 đã xác minh."
                    .into(),
            );
        }
        if self.buffer_count != super::as200::map::BUFFER_COUNT {
            return Err("Profile AS200 Published v1 chỉ xác minh 2 buffer do PLC quản lý; chưa hỗ trợ số buffer khác.".into());
        }
        if self.record_size != super::as200::map::RECORD_SIZE {
            return Err("Profile AS200 Published v1 chỉ xác minh 240 WORD; 256/300 WORD cần Register Map và quy tắc lưu raw tương ứng.".into());
        }
        self.memory_end()?;
        Ok(())
    }
    pub fn validate(&self) -> Result<(), String> {
        self.validate_read()?;
        if let Some(sku) = &self.operator_sku {
            crate::services::inbox_service::validate_operator_sku(sku)?;
        }
        if self.read_only
            && (self.ack_enabled || self.pc_heartbeat_enabled || self.allow_plc_heartbeat_write)
        {
            return Err("Chế độ chỉ đọc không cho phép ACK hoặc ghi heartbeat.".into());
        }
        if self.mode == "modbus" && !self.technical_setup_confirmed && !self.read_only {
            return Err("Chưa xác nhận thiết lập kỹ thuật; chưa cho phép thu dữ liệu".into());
        }
        if self
            .plc_heartbeat_timeout_ms
            .is_some_and(|v| !(2000..=3600000).contains(&v))
        {
            return Err("Heartbeat timeout ngoài phạm vi cấu hình".into());
        }
        if matches!(self.mode.as_str(), "plc" | "modbus")
            && self.pc_heartbeat_enabled
            && (!self.allow_plc_heartbeat_write || !self.allow_plc_connection)
        {
            return Err("Chưa cho phép ghi PC_Heartbeat trên PLC thật".into());
        }
        if self.ack_enabled
            && !match self.mode.as_str() {
                "plc" | "modbus" => {
                    self.allow_plc_connection && self.ack_protocol.as_deref() == Some("as200")
                }
                "simulator" => self.ack_protocol.as_deref() == Some("as200"),
                _ => false,
            }
        {
            return Err("ACK yêu cầu protocol AS200 đã được xác nhận".into());
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
        {
            return Err("Chu kỳ polling/xử lý Measurement ngoài phạm vi cấu hình".into());
        }
        Ok(())
    }
    pub fn reader(&self) -> As200Config {
        As200Config {
            network_adapter: self.plc_adapter.clone(),
            control_start: self.control_start,
            record_size: self.record_size,
            buffer_count: self.buffer_count,
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
    pub fn plc_reader(&self) -> super::as200::reader::As200Reader {
        super::as200::reader::As200Reader::new(self.reader())
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
    // An explicitly selected application directory is authoritative. Never fall back elsewhere.
    if app_directory.is_some() {
        return candidates.into_iter().find(|p| p.is_file());
    }
    // Standalone development helpers may still resolve their project working directory.
    if cfg!(debug_assertions)
        && let Ok(cwd) = std::env::current_dir()
    {
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
    let config = parse(&text).map_err(|error| {
        eprintln!(
            "AS200_CONFIG_REJECTED pid={} path={} reason={error}",
            std::process::id(),
            path.display()
        );
        error
    })?;
    let journal = config.journal_file(&path);
    let resolved_path = path.canonicalize().unwrap_or_else(|_| path.clone());
    eprintln!(
        "AS200_CONFIG_LOADED pid={} path={} address={} unit={} ack_enabled={} read_only={} technical_setup_confirmed={} allow_plc_connection={} source={} session={:?} journal={} ack_config_reason={}",
        std::process::id(), resolved_path.display(), config.address, config.unit_id,
        config.ack_enabled, config.read_only, config.technical_setup_confirmed,
        config.allow_plc_connection, config.source_key, config.confirmed_session_id,
        journal.display(), config.ack_config_block_reason().unwrap_or("Cấu hình ACK đủ điều kiện; vẫn phải xác minh binding/Session, RESET, snapshot, heartbeat và raw receipt PostgreSQL đã commit trước khi ghi")
    );
    Ok(Some((config, journal)))
}

pub(super) fn parse(text: &str) -> Result<SimulatorConfig, String> {
    let values: toml::Value =
        toml::from_str(text).map_err(|e| format!("Cấu hình AS200 không hợp lệ: {e}"))?;
    let config: SimulatorConfig = values
        .clone()
        .try_into()
        .map_err(|e| format!("Cấu hình AS200 không hợp lệ: {e}"))?;
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
        use crate::plc::as200::ack::AckWriter;
        assert!(AckWriter::new(cfg.reader(), "modbus", true, Some("as200"), false).is_err());
        assert!(AckWriter::new(cfg.reader(), "modbus", true, Some("as200"), true).is_ok());
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

#[cfg(test)]
mod runtime_profile_tests {
    use super::*;
    #[test]
    fn profile_defaults_translation_bounds_and_write_scope() {
        let mut c = SimulatorConfig {
            source_key: "fixture".into(),
            mode: "modbus".into(),
            technical_setup_confirmed: true,
            ack_protocol: Some("as200".into()),
            ..Default::default()
        };
        assert!(c.validate().is_ok());
        assert_eq!(c.memory_end().unwrap(), 25499);
        c.control_start = 26000;
        assert_eq!(c.memory_end().unwrap(), 26499);
        assert_eq!(c.reader().control_start, 26000);
        assert!(!ack_scope_allows(&c));
        c.modbus_address_offset = -1;
        c.word_order = StoredWordOrder::High;
        assert!(c.validate_read().is_ok());
        assert_eq!(c.reader().word_order, WordOrder::HighWordFirst);
        assert_eq!(c.reader().modbus_address_offset, -1);
        for (count, size) in [
            (1, 240),
            (3, 240),
            (2, 256),
            (2, 300),
            (usize::MAX, usize::MAX),
        ] {
            c.buffer_count = count;
            c.record_size = size;
            assert!(c.validate_layout().is_err());
        }
        c.buffer_count = 2;
        c.record_size = 240;
        c.control_start = 65036;
        c.modbus_address_offset = 0;
        assert!(c.validate_read().is_ok());
        c.control_start += 1;
        assert!(c.validate_read().is_err());
        c.control_start = 0;
        c.modbus_address_offset = -1;
        assert!(c.validate_read().is_err());
        c.control_start = 25000;
        c.modbus_address_offset = 0;
        c.word_order = StoredWordOrder::Low;
        assert!(ack_scope_allows(&c));
        c.read_only = true;
        c.ack_enabled = true;
        assert!(c.validate().is_err());
        assert!(!ack_scope_allows(&c));
    }
}
