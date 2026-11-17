//! One TOML-selected environment and one Worker. No live endpoint switching.
use super::{
    as200::{As200Error, map},
    simulator_config::{self, SimulatorConfig},
    worker::WorkerState,
};
use crate::{
    models::InboxInput,
    repositories::{InboxRepository, SourceRepository},
    services::InboxService,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    hash::{Hash, Hasher},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

pub(super) const PLC_ADDRESS: &str = "192.168.1.5:502";
pub(super) const SOURCE_KEY: &str = "golf-report-plc-01";
const JOURNAL: &str = "local-runtime/capture.json";
const SIMULATOR_SOURCE_KEY: &str = "golf-report-simulator";
const SIMULATOR_JOURNAL: &str = "local-runtime/simulator/capture.json";
const CONNECT: &str = "Không thể kết nối với PLC. Vui lòng kiểm tra mạng và nguồn PLC.";
const DEFAULT_CONFIG: &str = include_str!("../../../as200_config.example.toml");

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Endpoint {
    pub ip: String,
    pub port: u32,
    pub unit_id: u16,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeParameters {
    plc_type: &'static str,
    protocol: &'static str,
    layout_profile: String,
    control_start: u16,
    buffer_count: usize,
    record_size: usize,
    word_order: crate::models::StoredWordOrder,
    modbus_address_offset: i32,
    poll_interval_ms: u64,
    timeout_ms: u64,
    heartbeat_timeout_ms: Option<u64>,
    retry_interval_ms: u64,
    read_only: bool,
    ack_enabled: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    operator_sku: Option<String>,
    plc_local_ip: Option<std::net::IpAddr>,
    endpoint: Endpoint,
    parameters: RuntimeParameters,
    configured: bool,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectionReport {
    pub tcp_connected: bool,
    pub modbus_read_succeeded: bool,
    pub as200_valid: bool,
    pub raw_save_attempted: bool,
    pub raw_saved: bool,
    pub mapping_warning: Option<String>,
    pub message: String,
}

// Read only the transport settings. A broken Journal, session or ACK permission
// must not prevent inspecting raw FC03 words, and cannot grant write capability.
#[derive(Deserialize)]
struct RegisterReadConnection {
    address: String,
    unit_id: u8,
    plc_local_ip: Option<std::net::IpAddr>,
    modbus_address_offset: i32,
    timeout_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterWordsReport {
    endpoint: Endpoint,
    plc_local_ip: Option<std::net::IpAddr>,
    modbus_address_offset: i32,
    start_d: u16,
    words: Vec<u16>,
    read_at: chrono::DateTime<chrono::Utc>,
}

#[tauri::command]
pub async fn read_plc_register_words(
    start_d: u32,
    word_count: u32,
    state: tauri::State<'_, WorkerState>,
) -> Result<RegisterWordsReport, String> {
    use tokio_modbus::{Request, Response, Slave, client::{Client, tcp}};
    if word_count == 0 || word_count > map::FC03_LIMIT as u32
        || start_d > u16::MAX as u32 || start_d + word_count > 65536
    {
        return Err(format!("Nhập địa chỉ D từ 0–65535, số WORD từ 1–{}; vùng đọc không được vượt D65535.", map::FC03_LIMIT));
    }
    // Serialize UI/configuration operations without waiting behind a stop/start.
    // No readiness/write probe or Journal operation belongs in this command.
    let _settings_gate = state.settings_gate.try_lock().map_err(|_| "Đang có thao tác PLC khác; vui lòng thử lại.")?;
    let engine = state.engine_for_diagnostic_read().await;
    let _worker_gate = engine.as_ref().map(|engine| engine.try_diagnostic_read_lock()).transpose()?;
    {
        let status = state.status.read().await;
        if status.state == "STOPPING" || (status.enabled && status.pause_reason.is_none()) {
            return Err("Worker đang thu/kết nối PLC. Chưa đọc chẩn đoán để tránh ảnh hưởng chu kỳ sản xuất; chỉ đọc khi đã dừng hoặc Worker đang pause.".into());
        }
        if status.enabled && engine.is_none() {
            return Err("Worker chưa sẵn sàng cho đọc chẩn đoán; vui lòng thử lại.".into());
        }
    }
    let directory = state.settings_directory.lock().await.clone();
    let directory = match directory { Some(directory) => directory, None => crate::application_directory()? };
    let text = fs::read_to_string(directory.join("as200_config.toml"))
        .map_err(|_| "Không đọc được as200_config.toml để lấy cấu hình kết nối.")?;
    let config: RegisterReadConnection = toml::from_str(&text)
        .map_err(|_| "Cấu hình đọc PLC không hợp lệ. Kiểm tra address, unit_id, plc_local_ip, modbus_address_offset và timeout_ms.")?;
    let address: std::net::SocketAddr = config.address.parse().map_err(|_| "Địa chỉ PLC phải là IP:Port hợp lệ.")?;
    if address.port() == 0 || !(1..=247).contains(&config.unit_id) || !(100..=30000).contains(&config.timeout_ms) {
        return Err("Thông số đọc PLC không hợp lệ: Port 1–65535, Unit ID 1–247, timeout 100–30000 ms.".into());
    }
    let pdu_start = map::modbus_address(start_d as u16, config.modbus_address_offset)
        .ok_or("Offset Modbus làm địa chỉ đầu vượt phạm vi UINT16.")?;
    map::modbus_address((start_d + word_count - 1) as u16, config.modbus_address_offset)
        .ok_or("Offset Modbus làm địa chỉ cuối vượt phạm vi UINT16.")?;
    let duration = std::time::Duration::from_millis(config.timeout_ms);
    let stream = super::as200::connect_tcp(&config.address, config.plc_local_ip, duration)
        .await.map_err(|e| e.to_string())?;
    let mut context = tcp::attach_slave(stream, Slave(config.unit_id));
    // A single fixed FC03 request. There is no API to choose a write function,
    // decode a measurement, persist data, acknowledge or retry a PLC command.
    let response = tokio::time::timeout(duration,
        context.call(Request::ReadHoldingRegisters(pdu_start, word_count as u16)))
        .await.map_err(|_| "Hết thời gian chờ phản hồi FC03 từ PLC; chưa có kết quả đọc.")?
        .map_err(|e| format!("Không đọc được phản hồi Modbus TCP FC03: {e}"))?
        .map_err(|e| format!("PLC trả Modbus Exception FC03: {e}"))?;
    let Response::ReadHoldingRegisters(words) = response else {
        return Err("Phản hồi PLC không phải FC03 Read Holding Registers.".into());
    };
    if words.len() != word_count as usize {
        return Err(format!("Phản hồi FC03 thiếu/thừa WORD: yêu cầu {word_count}, nhận {}.", words.len()));
    }
    Ok(RegisterWordsReport {
        endpoint: Endpoint { ip: address.ip().to_string(), port: address.port().into(), unit_id: config.unit_id.into() },
        plc_local_ip: config.plc_local_ip, modbus_address_offset: config.modbus_address_offset,
        start_d: start_d as u16, words, read_at: chrono::Utc::now(),
    })
}

// Never rewrite an old profile or adopt old history under a new identity.
pub(super) fn validate_config(config: &SimulatorConfig) -> Result<(), String> {
    config.validate_read()?;
    let endpoint: std::net::SocketAddr = config
        .address
        .parse()
        .map_err(|_| "Address cần IP:port cụ thể")?;
    let simulator = endpoint.ip().is_loopback();
    let (source_key, journal) = if simulator {
        (SIMULATOR_SOURCE_KEY, SIMULATOR_JOURNAL)
    } else {
        (SOURCE_KEY, JOURNAL)
    };
    if config.mode != "modbus"
        || config.source_key != source_key
        || config.journal_path.as_deref() != Some(Path::new(journal))
        || config
            .confirmed_session_id
            .as_deref()
            .is_none_or(|s| s.trim().is_empty())
        || config
            .confirmed_session_id
            .as_deref()
            .is_some_and(|s| s.starts_with("simulator-") != simulator)
    {
        return Err("Cấu hình môi trường chưa tách biệt: Simulator dùng SourceKey=golf-report-simulator, Session bắt đầu simulator-, Journal=local-runtime/simulator/capture.json; PLC dùng SourceKey=golf-report-plc-01, Session không bắt đầu simulator-, Journal=local-runtime/capture.json. Không đổi danh tính của Journal cũ để vượt lỗi.".into());
    }
    if config.pc_heartbeat_enabled || config.allow_plc_heartbeat_write {
        return Err("Published Record không cho phép ghi PC Heartbeat.".into());
    }
    if config.ack_enabled {
        if let Some(reason) = config.ack_config_block_reason() {
            return Err(format!("ACK bị chặn trong as200_config.toml: {reason}."));
        }
    }
    Ok(())
}

fn has_prior_runtime(directory: &Path) -> Result<bool, String> {
    // Recognize old evidence without retaining any source selection/switching logic.
    if [
        "as200_capture_journal.json",
        "as200_capture_journal.lock",
        "as200_config.connection-pending",
        "as200_config.connection-draft",
    ]
    .iter()
    .any(|name| directory.join(name).exists())
    {
        return Ok(true);
    }
    let runtime = directory.join("local-runtime");
    if !runtime.exists() {
        return Ok(false);
    }
    Ok(fs::read_dir(runtime)
        .map_err(|_| "Không đọc được runtime PLC.")?
        .next()
        .is_some())
}

pub(crate) fn ensure_config(directory: &Path) -> Result<(), String> {
    let path = directory.join("as200_config.toml");
    if path.exists() {
        return Ok(());
    }
    if has_prior_runtime(directory)? {
        return Err("Thiếu cấu hình nhưng còn lịch sử/Journal PLC. Không tự tạo cấu hình hoặc Session thay thế; hãy khôi phục bản sao lưu.".into());
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "Không tạo được as200_config.toml. Kiểm tra quyền ghi thư mục ứng dụng.")?;
    file.write_all(DEFAULT_CONFIG.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|_| "Không lưu được cấu hình PLC; chưa bật thu/ACK.".into())
}

fn read_config(directory: &Path) -> Result<(PathBuf, String, SimulatorConfig), String> {
    let path = directory.join("as200_config.toml");
    let text = fs::read_to_string(&path).map_err(|_| "Không đọc được as200_config.toml.")?;
    let config = simulator_config::parse(&text)?;
    validate_config(&config)?;
    Ok((path, text, config))
}

async fn current_config(
    state: &WorkerState,
    directory: &Path,
) -> Result<(PathBuf, String, SimulatorConfig), String> {
    let loaded = read_config(directory)?;
    state.require_startup_environment(&loaded.2).await?;
    Ok(loaded)
}

async fn directory(state: &WorkerState) -> Result<PathBuf, String> {
    state
        .settings_directory
        .lock()
        .await
        .clone()
        .ok_or_else(|| "Chưa xác định thư mục ứng dụng.".into())
}
async fn ready(state: &WorkerState) -> Result<PathBuf, String> {
    if let Some(error) = state.runtime_error.lock().await.clone() {
        return Err(error);
    }
    if !state.settings_ready.load(Ordering::SeqCst) {
        return Err("Runtime PLC chưa sẵn sàng.".into());
    }
    let dir = directory(state).await?;
    crate::check_plc_directory(&dir)?;
    Ok(dir)
}

fn settings(config: &SimulatorConfig) -> Settings {
    let address: std::net::SocketAddr = config.address.parse().expect("validated PLC address");
    Settings {
        operator_sku: config.operator_sku.clone(),
        plc_local_ip: config.plc_local_ip,
        endpoint: Endpoint {
            ip: address.ip().to_string(),
            port: address.port().into(),
            unit_id: config.unit_id.into(),
        },
        parameters: RuntimeParameters {
            // validate_config restricts this DTO to the AS200 Modbus profile.
            plc_type: "Delta AS200 Series",
            protocol: "Modbus TCP",
            layout_profile: config.layout_profile.clone(),
            control_start: config.control_start,
            buffer_count: config.buffer_count,
            record_size: config.record_size,
            word_order: config.word_order,
            modbus_address_offset: config.modbus_address_offset,
            poll_interval_ms: config.poll_interval_ms,
            timeout_ms: config.timeout_ms,
            heartbeat_timeout_ms: config.plc_heartbeat_timeout_ms,
            retry_interval_ms: config.retry_interval_ms,
            read_only: config.read_only,
            ack_enabled: config.ack_enabled,
        },
        configured: true,
    }
}

// This changes only enabled, retaining the configured identity/session and ACK permissions.
fn enable(path: &Path, before: &str) -> Result<(), String> {
    let mut document: toml::Value =
        toml::from_str(before).map_err(|_| "Cấu hình PLC không hợp lệ.")?;
    document
        .as_table_mut()
        .ok_or("Cấu hình PLC không hợp lệ.")?
        .insert("enabled".into(), true.into());
    let after = toml::to_string_pretty(&document).map_err(|_| "Không tạo được cấu hình PLC.")?;
    replace_config(path, before, &after)
}

fn replace_config(path: &Path, before: &str, after: &str) -> Result<(), String> {
    if fs::read_to_string(path).map_err(|_| "Không đọc được cấu hình PLC.")? != before {
        return Err("Cấu hình đã thay đổi; vui lòng thử lại.".into());
    }
    let tmp = path.with_extension("tmp");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|_| "Không tạo được cấu hình tạm; giữ nguyên file hiện có.")?;
    file.write_all(after.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|_| "Không lưu được cấu hình PLC.")?;
    drop(file);
    if fs::read_to_string(path).map_err(|_| "Không đọc được cấu hình PLC.")? != before {
        let _ = fs::remove_file(&tmp);
        return Err("Cấu hình đã thay đổi; vui lòng thử lại.".into());
    }
    fs::rename(tmp, path)
        .map_err(|_| "Không thay thế được cấu hình PLC. Kiểm tra quyền ghi và thử lại.".into())
}

// A missing Journal is never interpreted as a clean PLC merely because its source key changed.
pub(super) async fn prepare_collection(
    directory: &Path,
    config: &SimulatorConfig,
    pool: &sqlx::PgPool,
) -> Result<(), String> {
    validate_config(config)?;
    if directory.join("as200_config.connection-pending").exists() {
        return Err("Còn thay đổi cấu hình cũ chưa được đối chiếu. Giữ nguyên Journal và xử lý trạng thái đang chờ trước khi thu/ACK.".into());
    }
    if config
        .journal_file(&directory.join("as200_config.toml"))
        .exists()
    {
        return Ok(());
    }
    // Other environment's Journal is retained, not interpreted as this source's history.
    // Unknown legacy markers remain blocking evidence.
    let journal = config.journal_file(&directory.join("as200_config.toml"));
    let parent = journal.parent().ok_or("Đường dẫn Journal không hợp lệ.")?;
    let legacy = ["as200_capture_journal.json", "as200_capture_journal.lock",
        "as200_config.connection-pending", "as200_config.connection-draft"]
        .iter().any(|name| directory.join(name).exists());
    let prior = if parent.exists() {
        let mut prior = false;
        for entry in fs::read_dir(parent).map_err(|_| "Không đọc được runtime PLC.")? {
            let entry = entry.map_err(|_| "Không đọc được runtime PLC.")?;
            if parent == directory.join("local-runtime")
                && entry.file_name() == "simulator"
                && entry.path().is_dir()
            {
                continue;
            }
            prior = true;
        }
        prior
    } else {
        false
    };
    if legacy || prior {
        return Err("Còn lịch sử PLC cũ nhưng thiếu Journal đang hoạt động. Giữ nguyên dữ liệu và đối chiếu trước khi khởi tạo.".into());
    }
    let history: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM plc_sources WHERE source_key=$1) OR EXISTS(SELECT 1 FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id WHERE s.source_key=$1) OR EXISTS(SELECT 1 FROM measurements m JOIN plc_sources s ON s.id=m.source_id WHERE s.source_key=$1)",
    )
    .bind(&config.source_key)
    .fetch_one(pool)
    .await
    .map_err(|_| "Không kiểm tra được lịch sử PostgreSQL; chưa tạo Journal.")?;
    if history {
        return Err("Thiếu Journal nhưng PostgreSQL còn lịch sử của SourceKey đang cấu hình. Khôi phục/đối chiếu Journal; không tự tạo Session hay checkpoint.".into());
    }
    read_new_baseline(config).await
}

#[tauri::command]
pub async fn get_plc_connection_settings(
    state: tauri::State<'_, WorkerState>,
) -> Result<Settings, String> {
    let dir = directory(&state).await?;
    let (_, _, config) = current_config(&state, &dir).await?;
    Ok(settings(&config))
}

#[tauri::command]
pub async fn save_plc_poll_interval(
    poll_interval_ms: u64,
    state: tauri::State<'_, WorkerState>,
) -> Result<Settings, String> {
    save_connection_options(poll_interval_ms, None, state).await
}

#[tauri::command]
pub async fn save_plc_connection_options(
    poll_interval_ms: u64,
    plc_local_ip: String,
    state: tauri::State<'_, WorkerState>,
) -> Result<Settings, String> {
    save_connection_options(poll_interval_ms, Some(plc_local_ip), state).await
}

async fn save_connection_options(
    poll_interval_ms: u64,
    plc_local_ip: Option<String>,
    state: tauri::State<'_, WorkerState>,
) -> Result<Settings, String> {
    if !(50..=60000).contains(&poll_interval_ms) {
        return Err("Chu kỳ đọc phải là số nguyên từ 50 đến 60000 ms.".into());
    }
    let _gate = state.settings_gate.lock().await;
    let dir = ready(&state).await?;
    let (path, before, config) = current_config(&state, &dir).await?;
    let mut document: toml::Value =
        toml::from_str(&before).map_err(|_| "Cấu hình PLC không hợp lệ.")?;
    document
        .as_table_mut()
        .ok_or("Cấu hình PLC không hợp lệ.")?
        .insert("poll_interval_ms".into(), (poll_interval_ms as i64).into());
    if let Some(value) = plc_local_ip {
        let local_ip: Option<std::net::IpAddr> = if value.trim().is_empty() { None } else {
            Some(value.trim().parse().map_err(|_| "IP nguồn PLC không hợp lệ. Nhập IP Ethernet, ví dụ 192.168.1.100, hoặc để trống.")?)
        };
        if local_ip != config.plc_local_ip {
            state.ensure_register_writes_stopped().await
                .map_err(|_| "Cần ngắt kết nối và đợi Worker dừng hoàn toàn trước khi đổi IP nguồn PLC. Không tự dừng hoặc gửi lại ACK.".to_string())?;
            if let Some(ip) = local_ip {
                // Bind only: validate local assignment without connecting or writing PLC.
                let endpoint = config.address.parse().map_err(|_| "Endpoint PLC không hợp lệ.")?;
                drop(super::as200::bound_socket(endpoint, ip)?);
            }
        }
        let table = document.as_table_mut().ok_or("Cấu hình PLC không hợp lệ.")?;
        match local_ip {
            Some(ip) => { table.insert("plc_local_ip".into(), ip.to_string().into()); }
            None => { table.remove("plc_local_ip"); }
        }
    }
    let after = toml::to_string_pretty(&document).map_err(|_| "Không tạo được cấu hình PLC.")?;
    let saved = simulator_config::parse(&after)?;
    validate_config(&saved)?;
    replace_config(&path, &before, &after)?;
    // Running Engine retains its immutable config. The next start loads this value;
    // saving never stops/restarts the Worker or changes an ACK intent/checkpoint.
    Ok(settings(&saved))
}

#[tauri::command]
pub async fn save_operator_sku(
    sku: String,
    state: tauri::State<'_, WorkerState>,
    database: tauri::State<'_, crate::database::DatabaseState>,
) -> Result<Settings, String> {
    let sku = sku.trim().to_string();
    crate::services::inbox_service::validate_operator_sku(&sku)?;
    let _gate = state.settings_gate.lock().await;
    let dir = ready(&state).await?;
    let (path, before, config) = current_config(&state, &dir).await?;
    if config.operator_sku.as_deref() == Some(sku.as_str()) { return Ok(settings(&config)); }
    let pool = database.pool().await?;
    let _journal_lock = state.verify_sku_change(&dir, &config, &pool).await?;
    let mut document: toml::Value = toml::from_str(&before).map_err(|_| "Cấu hình PLC không hợp lệ.")?;
    document.as_table_mut().ok_or("Cấu hình PLC không hợp lệ.")?.insert("operator_sku".into(), sku.into());
    let after = toml::to_string_pretty(&document).map_err(|_| "Không tạo được cấu hình SKU.")?;
    let saved = simulator_config::parse(&after)?;
    validate_config(&saved)?;
    replace_config(&path, &before, &after)?;
    Ok(settings(&saved))
}
#[tauri::command]
pub async fn test_plc_connection(
    state: tauri::State<'_, WorkerState>,
    database: tauri::State<'_, crate::database::DatabaseState>,
) -> Result<InspectionReport, String> {
    let _gate = state.settings_gate.lock().await;
    let dir = ready(&state).await?;
    let (_, _, config) = current_config(&state, &dir).await?;
    let save_raw = !config.journal_file(&dir.join("as200_config.toml")).exists();
    let pool = database.pool().await.ok();
    Ok(inspect_endpoint_with_raw(config, pool.as_ref(), save_raw).await)
}

#[tauri::command]
pub async fn connect_plc_collection(
    state: tauri::State<'_, WorkerState>,
    database: tauri::State<'_, crate::database::DatabaseState>,
) -> Result<(), String> {
    let _gate = state.settings_gate.lock().await;
    let dir = ready(&state).await?;
    let (path, text, config) = current_config(&state, &dir).await?;
    if state.status.read().await.enabled {
        return Ok(());
    }
    config.validate()?;
    let endpoint = config.address.clone();
    let journal_exists = config.journal_file(&path).exists();
    let save_raw = !journal_exists;
    let pool = database.pool().await?;
    let report = inspect_endpoint_with_raw(config.clone(), Some(&pool), save_raw).await;
    if !report.as200_valid {
        state.record_connection("FAILED", &endpoint).await;
        return Err(report.mapping_warning.unwrap_or(report.message));
    }
    if report.raw_save_attempted {
        return Err(format!(
            "{} Chưa khởi tạo Journal production và chưa bật thu/ACK.",
            report.message
        ));
    }
    WorkerState::require_sku_or_recovery(&config, &config.journal_file(&path))?;
    // Persist the operator's request, never an ACK permission or replacement session.
    enable(&path, &text)?;
    let marker = path.with_extension("operator-stop");
    if marker.exists() {
        fs::remove_file(marker).map_err(|_| "Không gỡ được yêu cầu dừng; chưa kết nối.")?;
    }
    state
        .clone()
        .inner()
        .clone()
        .launch_under_gate(Some(dir), database.inner().clone())
        .await;
    let status = state.status.read().await;
    if !status.enabled {
        return Err(status.message.clone());
    }
    Ok(())
}
#[tauri::command]
pub async fn stop_plc_collection(state: tauri::State<'_, WorkerState>) -> Result<(), String> {
    let _gate = state.settings_gate.lock().await;
    // A changed/invalid config must never prevent stopping the already running Worker.
    state.stop_collection().await
}

#[tauri::command]
pub async fn clear_plc_connection_log(state: tauri::State<'_, WorkerState>) -> Result<(), String> {
    state.clear_connection_log().await
}

async fn inspect_endpoint(config: SimulatorConfig) -> InspectionReport {
    inspect_endpoint_with_raw(config, None, false).await
}

async fn inspect_endpoint_with_raw(
    config: SimulatorConfig,
    pool: Option<&sqlx::PgPool>,
    save_raw_if_uninitialized: bool,
) -> InspectionReport {
    let mut report = InspectionReport {
        tcp_connected: false,
        modbus_read_succeeded: false,
        as200_valid: false,
        raw_save_attempted: false,
        raw_saved: false,
        mapping_warning: None,
        message: CONNECT.into(),
    };
    if let Err(error) = config.validate_read() {
        report.mapping_warning = Some(error);
        report.message =
            "Cấu hình vùng đọc chưa hợp lệ. Kiểm tra Register Map trước khi thử lại.".into();
        return report;
    }
    // Separate TCP reachability from a valid FC03 exchange. Both sockets are closed
    // after this one-shot inspection; no Worker or PLC writer is constructed.
    match super::as200::connect_tcp(&config.address, config.plc_local_ip,
        std::time::Duration::from_millis(config.timeout_ms)).await {
        Ok(stream) => {
            report.tcp_connected = true;
            drop(stream);
        }
        Err(error) => {
            report.message = error.to_string();
            return report;
        }
    }
    match tokio::time::timeout(
        std::time::Duration::from_millis(config.timeout_ms.saturating_mul(10)),
        config.plc_reader().poll_published_record(),
    )
    .await
    {
        Ok(Ok(snapshot)) => {
            report.modbus_read_succeeded = true;
            report.mapping_warning = as200_structure(&snapshot).err();
            report.as200_valid = report.mapping_warning.is_none();
            if save_raw_if_uninitialized && snapshot.control.data_ready == 1 && report.as200_valid {
                report.raw_save_attempted = true;
                match pool {
                    Some(pool) => match save_uninitialized_raw(pool, &config, &snapshot).await {
                        Ok(_) => report.raw_saved = true,
                        Err(error) => report.mapping_warning = Some(error),
                    },
                    None => {
                        report.mapping_warning =
                            Some("Không có kết nối PostgreSQL để lưu raw PLC.".into());
                    }
                }
            }
            if report.as200_valid && !snapshot.is_zero_test() && !snapshot.diagnostics.is_empty() {
                report.mapping_warning = Some(format!("Bản ghi chưa đủ điều kiện giải mã/ACK: {:?}", snapshot.diagnostics));
            }
            report.message = if report.raw_save_attempted && report.raw_saved {
                "Đọc PLC thành công; đã lưu raw 240 WORD vào Inbox ở trạng thái chưa xác nhận Session. Chưa đủ điều kiện thu dữ liệu production hoặc ACK."
            } else if report.raw_save_attempted {
                "Đọc PLC thành công nhưng chưa lưu được raw vào PostgreSQL; chưa đủ điều kiện thu dữ liệu production hoặc ACK."
            } else if report.as200_valid && snapshot.is_zero_test() {
                "TCP/Modbus và control AS200 hợp lệ; PLC đang công bố 240 WORD toàn 0. Raw đã nhận nhưng chưa đủ định danh để tạo measurement hoặc ACK."
            } else if report.as200_valid && !snapshot.diagnostics.is_empty() {
                "TCP/Modbus và control AS200 hợp lệ; bản ghi chưa hoàn chỉnh. Worker chỉ giữ raw và chờ dữ liệu hợp lệ, chưa tạo measurement hoặc ACK. Lần kiểm tra này không lưu dữ liệu."
            } else if report.as200_valid {
                "TCP và Modbus thành công; dữ liệu AS200 hợp lệ trong lần kiểm tra."
            } else {
                "Đã đọc Modbus; dữ liệu AS200 chưa hợp lệ. Kiểm tra Register Map và trạng thái PLC."
            }
            .into();
        }
        Ok(Err(As200Error::InvalidData(error))) => {
            report.modbus_read_succeeded = true;
            report.mapping_warning = Some(error);
            report.message = "Đã đọc Modbus; Register Map/dữ liệu AS200 chưa hợp lệ. Vui lòng kiểm tra cấu hình PLC.".into();
        }
        Ok(Err(As200Error::Unstable)) => {
            report.modbus_read_succeeded = true;
            report.mapping_warning =
                Some("Dữ liệu PLC thay đổi trong lần kiểm tra; chưa có snapshot ổn định.".into());
            report.message =
                "Modbus đã phản hồi; dữ liệu chưa ổn định. Vui lòng kiểm tra lại.".into();
        }
        Ok(Err(error)) => {
            report.message = "TCP đã kết nối nhưng chưa đọc được Modbus hợp lệ. Kiểm tra Unit ID, vùng nhớ và timeout.".into();
            report.mapping_warning = Some(error.to_string());
        }
        Err(_) => {
            report.message = "TCP đã kết nối nhưng chưa đọc được Modbus hợp lệ. Kiểm tra Unit ID, vùng nhớ và timeout.".into();
        }
    }
    report
}

async fn save_uninitialized_raw(
    pool: &sqlx::PgPool,
    config: &SimulatorConfig,
    snapshot: &super::as200::reader::PublishedRecordSnapshot,
) -> Result<(), String> {
    let buffer = snapshot
        .buffer
        .as_ref()
        .filter(|buffer| buffer.buffer_index == 0 && buffer.raw_registers.len() == map::RECORD_SIZE)
        .ok_or("DataReady=1 nhưng không có đủ 240 WORD để lưu raw.")?;
    let raw = serde_json::to_value(&buffer.raw_registers).map_err(|e| e.to_string())?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    snapshot.control.total_count.hash(&mut hasher);
    snapshot.control.pc_read_count.hash(&mut hasher);
    snapshot.control.pc_read_done.hash(&mut hasher);
    buffer.raw_registers.hash(&mut hasher);
    let base_key = format!(
        "{}{}:{:016x}",
        crate::services::inbox_service::RAW_UNINITIALIZED_KEY,
        snapshot.control.total_count,
        hasher.finish()
    );
    let source = SourceRepository::upsert(pool, &config.source_key, None).await?;
    let record_seq = super::as200::decode::dint32(
        &buffer.raw_registers,
        map::record::SEQUENCE,
        config.reader().word_order,
    )
    .map_err(|e| e.to_string())?;
    let total_count = Some(i64::from(snapshot.control.total_count));
    let existing = InboxRepository::existing_delivery(
        pool,
        source.id,
        &InboxInput {
            source_id: source.id,
            confirmed_session_id: None,
            ingest_key: base_key.clone(),
            record_seq,
            total_count,
            buffer_index: 0,
            raw_registers: buffer.raw_registers.clone(),
            word_order: config.word_order,
            received_at: Utc::now(),
        },
    )
    .await?;
    if let Some(row) = existing {
        if row.raw_registers == raw
            && row.record_seq == record_seq
            && row.total_count == total_count
            && row.buffer_index == 0
            && row.word_order == config.word_order.as_str()
        {
            return Ok(());
        }
        return Err(
            "Đã có raw chưa xác nhận khác cùng checkpoint/hash; giữ nguyên để đối chiếu thủ công."
                .into(),
        );
    }
    InboxService::receive(
        pool,
        &InboxInput {
            source_id: source.id,
            confirmed_session_id: None,
            ingest_key: base_key,
            record_seq,
            total_count,
            buffer_index: 0,
            raw_registers: buffer.raw_registers.clone(),
            word_order: config.word_order,
            received_at: Utc::now(),
        },
    )
    .await?;
    Ok(())
}

fn as200_structure(report: &super::as200::reader::PublishedRecordSnapshot) -> Result<(), String> {
    {
        let control = &report.control;
        if (control.data_ready == 1 && report.buffer.is_none())
            || report
                .buffer
                .as_ref()
                .is_some_and(|b| b.buffer_index != 0 || b.raw_registers.len() != map::RECORD_SIZE)
        {
            return Err("AS200 cần vùng Bản ghi 1 hợp lệ, đúng 240 WORD khi DataReady=1.".into());
        }
        if control.total_count < 0
            || control.pc_read_count < 0
            || control.pc_read_count > control.total_count
            || !matches!(control.data_ready, 0 | 1)
            || !matches!(control.pc_read_done, 0 | 1)
        {
            return Err(format!(
                "Trạng thái AS200 không hợp lệ: TotalCount={}, PC_ReadCount={}, DataReady={}, PC_ReadDone={}.",
                control.total_count,
                control.pc_read_count,
                control.data_ready,
                control.pc_read_done
            ));
        }
        // Stable FC03/control validates the transport/profile, not a measurement.
        // Record diagnostics are reported separately and cannot grant ACK.
        Ok(())
    }
}

pub(super) fn clean_new_baseline(
    report: &super::as200::reader::PublishedRecordSnapshot,
) -> Result<(), String> {
    let c = &report.control;
    // A new Journal has no accepted prefix. Never import a remote checkpoint.
    // DataReady=0 makes stale record words irrelevant. Payload contents do not
    // establish an ACK checkpoint; Worker checks the real counter before ACK.
    let published = c.data_ready == 1 && c.total_count >= 0
        && report.buffer.as_ref().is_some_and(|buffer| buffer.buffer_index == 0
            && buffer.raw_registers.len() == map::RECORD_SIZE);
    let idle = c.data_ready == 0 && c.total_count == 0 && report.buffer.is_none();
    if c.format_version != map::FORMAT_VERSION || c.record_size != map::RECORD_SIZE as i16
        || c.buffer_count != map::BUFFER_COUNT as i16 || c.pc_read_count != 0
        || c.pc_read_done != 0
        || !(idle || published)
    {
        return Err(format!(
            "Chưa chứng minh được điểm bắt đầu thu an toàn: TotalCount={}, PC_ReadCount={}, DataReady={}, PC_ReadDone={}. Journal mới không nhập checkpoint hoặc bỏ qua lịch sử ACK của PLC.",
            c.total_count, c.pc_read_count, c.data_ready, c.pc_read_done
        ));
    }
    Ok(())
}

pub(super) async fn read_new_baseline(config: &SimulatorConfig) -> Result<(), String> {
    let mut reader = config.plc_reader();
    let snapshot = reader.poll_published_record().await.map_err(|e| e.to_string())?;
    // The reader verifies stable control and, only when ready, all 240 raw words.
    clean_new_baseline(&snapshot)
}

#[cfg(test)]
mod tests;
