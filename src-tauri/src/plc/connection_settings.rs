//! Controlled QC connection lifecycle with journal/inbox and source continuity checks.
use super::{
    as200::{As200Error, map, reader::As200Reader},
    simulator_config::{self, SimulatorConfig},
    worker::{WorkerState, switch_log},
};
use crate::models::StoredWordOrder;
use serde::{Deserialize, Serialize};
use std::{
    hash::{Hash, Hasher},
    io::Write,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

const BUSY: &str =
    "Chưa thể thay đổi kết nối vì hệ thống đang xử lý dữ liệu đo. Vui lòng liên hệ kỹ thuật.";
const CONFIG: &str = "Chưa có cấu hình kết nối phù hợp. Vui lòng liên hệ kỹ thuật để thiết lập.";
const SAVE: &str = "Không thể lưu thay đổi. Vui lòng thử lại hoặc liên hệ kỹ thuật.";
const CONNECT: &str = "Không thể kết nối với PLC. Vui lòng kiểm tra địa chỉ IP và cổng kết nối.";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Endpoint {
    pub ip: String,
    pub port: u32,
    pub unit_id: u16,
}

fn managed_source_identity(endpoint: &Endpoint) -> (String, String) {
    let identity = format!("{}:{}:{}", endpoint.ip, endpoint.port, endpoint.unit_id);
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    identity.hash(&mut hash);
    let suffix = format!("{:016x}", hash.finish());
    (format!("as200-{suffix}"), format!("as200-{suffix}-session"))
}

async fn ensure_managed_source(
    pool: &sqlx::PgPool,
    endpoint: &Endpoint,
) -> Result<(String, String), String> {
    let (source_key, session_id) = managed_source_identity(endpoint);
    sqlx::query(
        "INSERT INTO plc_sources (source_key, description) VALUES ($1, $2) \
         ON CONFLICT (source_key) DO NOTHING",
    )
    .bind(&source_key)
    .bind(format!(
        "PLC AS200 {}:{} Unit {} (managed by Golf Report)",
        endpoint.ip, endpoint.port, endpoint.unit_id
    ))
    .execute(pool)
    .await
    .map_err(|_| "Không thể tạo nhận dạng nội bộ cho nguồn PLC trong PostgreSQL.".to_string())?;
    Ok((source_key, session_id))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitialTechnicalSetup {
    pub word_order: StoredWordOrder,
    pub modbus_address_offset: i32,
    pub plc_heartbeat_timeout_ms: u64,
    pub new_source_confirmed: bool,
    pub register_map_confirmed: bool,
    pub ack_confirmed: bool,
}
impl Endpoint {
    fn apply(&self, config: &SimulatorConfig) -> Result<SimulatorConfig, String> {
        let ip: IpAddr = self
            .ip
            .parse()
            .map_err(|_| "Địa chỉ IP chưa hợp lệ. Vui lòng kiểm tra lại.")?;
        if !(1..=65535).contains(&self.port) {
            return Err("Cổng kết nối phải từ 1 đến 65535.".into());
        }
        if !(1..=247).contains(&self.unit_id) {
            return Err("Unit ID phải từ 1 đến 247.".into());
        }
        let mut next = config.clone();
        next.address = SocketAddr::new(ip, self.port as u16).to_string();
        next.unit_id = self.unit_id as u8;
        next.validate_read().map_err(|e| {
            eprintln!("PLC settings policy: {e}");
            CONFIG.to_string()
        })?;
        Ok(next)
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub endpoint: Option<Endpoint>,
    pub configured: bool,
    pub technical_ready: bool,
    pub active_endpoint: Option<Endpoint>,
    pub revision: String,
    pub worker_running: bool,
    pub restart_required: bool,
}
fn revision(text: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    format!("{:016x}", h.finish())
}
fn set_document_value(
    document: &mut toml::Value,
    key: &str,
    value: toml::Value,
) -> Result<(), String> {
    document
        .as_table_mut()
        .ok_or_else(|| "Cấu hình AS200 phải là một TOML table.".to_string())?
        .insert(key.to_string(), value);
    Ok(())
}
fn read(path: &Path) -> Result<(String, SimulatorConfig), String> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        eprintln!("PLC settings read {}: {e}", path.display());
        CONFIG.to_string()
    })?;
    let config = simulator_config::parse(&text)
        .and_then(|c| {
            c.validate_read()?;
            Ok(c)
        })
        .map_err(|e| {
            eprintln!("PLC settings config: {e}");
            CONFIG.to_string()
        })?;
    Ok((text, config))
}
fn endpoint(config: &SimulatorConfig) -> Result<Endpoint, String> {
    let addr: SocketAddr = config.address.parse().map_err(|_| CONFIG.to_string())?;
    Ok(Endpoint {
        ip: addr.ip().to_string(),
        port: u32::from(addr.port()),
        unit_id: u16::from(config.unit_id),
    })
}
async fn path(state: &WorkerState) -> Result<PathBuf, String> {
    if !state.settings_ready.load(Ordering::SeqCst) {
        return Err("Đang chuẩn bị cấu hình kết nối. Vui lòng thử lại sau.".into());
    }
    state
        .settings_path
        .lock()
        .await
        .clone()
        .ok_or(CONFIG.to_string())
}
// Spans edit exactly the two root values. Comments, whitespace, unknown fields and tables survive.
fn patch(text: &str, next: &SimulatorConfig) -> Result<String, String> {
    #[derive(Deserialize)]
    struct Fields {
        address: toml::Spanned<String>,
        unit_id: toml::Spanned<u16>,
    }
    let fields: Fields = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut changes = vec![
        (fields.address.span(), format!("\"{}\"", next.address)),
        (fields.unit_id.span(), next.unit_id.to_string()),
    ];
    changes.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut result = text.to_string();
    for (range, value) in changes {
        result.replace_range(range, &value);
    }
    let before: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut expected = before;
    expected["address"] = toml::Value::String(next.address.clone());
    expected["unit_id"] = toml::Value::Integer(next.unit_id.into());
    let actual: toml::Value = toml::from_str(&result).map_err(|e| e.to_string())?;
    if actual != expected {
        return Err("TOML fields changed outside endpoint".into());
    }
    Ok(result)
}
fn replace(path: &Path, before: &str, after: &str) -> Result<(), String> {
    let config_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("settings.lock"))
        .map_err(|e| e.to_string())?;
    config_lock.try_lock().map_err(|e| e.to_string())?;
    let temp = path.with_extension(format!(
        "connection-{}-{}.tmp",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        file.write_all(after.as_bytes())
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        if std::fs::read_to_string(path).map_err(|e| e.to_string())? != before {
            return Err("Configuration changed during save".into());
        }
        std::fs::rename(&temp, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}
async fn get(state: &WorkerState) -> Result<Settings, String> {
    if let Some(error) = state.runtime_error.lock().await.clone() {
        return Err(error);
    }
    if state.settings_ready.load(Ordering::SeqCst) && state.settings_path.lock().await.is_none() {
        return Ok(Settings {
            endpoint: None,
            configured: false,
            technical_ready: false,
            active_endpoint: None,
            revision: "unconfigured".into(),
            worker_running: state.status.read().await.enabled,
            restart_required: false,
        });
    }
    let file = path(state).await?;
    let (text, config) = read(&file)?;
    let status = state.status.read().await;
    let active_endpoint = if status.enabled {
        status
            .address
            .parse::<SocketAddr>()
            .ok()
            .map(|addr| Endpoint {
                ip: addr.ip().to_string(),
                port: u32::from(addr.port()),
                unit_id: u16::from(status.unit_id),
            })
    } else {
        None
    };
    Ok(Settings {
        endpoint: Some(endpoint(&config)?),
        configured: true,
        technical_ready: config.validate().is_ok(),
        active_endpoint,
        revision: revision(&text),
        worker_running: status.enabled,
        restart_required: state.settings_restart.load(Ordering::SeqCst),
    })
}
async fn save(
    state: &WorkerState,
    value: Endpoint,
    expected_revision: &str,
) -> Result<Settings, String> {
    if let Some(error) = state.runtime_error.lock().await.clone() {
        return Err(error);
    }
    if state.settings_ready.load(Ordering::SeqCst) && state.settings_path.lock().await.is_none() {
        if expected_revision != "unconfigured" || state.status.read().await.enabled {
            return Err(BUSY.into());
        }
        let dir = state
            .settings_directory
            .lock()
            .await
            .clone()
            .ok_or(SAVE.to_string())?;
        if simulator_config::locate(Some(&dir)).is_some() {
            return Err(
                "Thông tin kết nối đã thay đổi. Vui lòng mở lại ứng dụng trước khi lưu.".into(),
            );
        }
        if cfg!(debug_assertions)
            && let Ok(cwd) = std::env::current_dir()
        {
            let mut folders = vec![cwd.clone(), cwd.join("src-tauri")];
            if cwd.file_name().is_some_and(|n| n == "src-tauri") {
                if let Some(parent) = cwd.parent() {
                    folders.push(parent.to_path_buf());
                }
            }
            if folders.iter().any(|folder| {
                folder.join("as200_capture_journal.json").exists()
                    || folder.join("as200_capture_journal.lock").exists()
            }) {
                return Err(BUSY.into()); // Missing config is not proof of a clean installation.
            }
        }
        let created = create_initial(&dir, &value).map_err(|e| {
            eprintln!("PLC initial setup: {e}");
            SAVE.to_string()
        })?;
        *state.settings_path.lock().await = Some(created);
        state.settings_restart.store(true, Ordering::SeqCst);
        return get(state).await;
    }
    let file = path(state).await?;
    let (before, config) = read(&file)?;
    if file.with_extension("connection-pending").exists() {
        return Err("Chuyển kết nối trước chưa hoàn tất. Vui lòng liên hệ kỹ thuật.".into());
    }
    if revision(&before) != expected_revision {
        return Err("Thông tin kết nối đã thay đổi. Vui lòng tải lại trang trước khi lưu.".into());
    }
    let next = value.apply(&config)?;
    if endpoint(&next)? == endpoint(&config)? {
        return get(state).await;
    }
    let _journal_lock = state
        .ensure_connection_edit_safe(&file, &config)
        .await
        .map_err(|e| {
            eprintln!("PLC settings save blocked: {e}");
            BUSY.to_string()
        })?;
    let after = patch(&before, &next)
        .and_then(|text| {
            let parsed = simulator_config::parse(&text)?;
            if parsed.enabled
                || parsed.ack_enabled
                || parsed.pc_heartbeat_enabled
                || parsed.technical_setup_confirmed
            {
                parsed.validate()?;
            } else {
                parsed.validate_read()?;
            }
            Ok(text)
        })
        .map_err(|e| {
            eprintln!("PLC settings patch: {e}");
            SAVE.to_string()
        })?;
    replace(&file, &before, &after).map_err(|e| {
        eprintln!("PLC settings save {}: {e}", file.display());
        SAVE.to_string()
    })?;
    state.settings_restart.store(true, Ordering::SeqCst);
    Ok(Settings {
        endpoint: Some(endpoint(&next)?),
        configured: true,
        technical_ready: next.validate().is_ok(),
        active_endpoint: None,
        revision: revision(&after),
        worker_running: false,
        restart_required: true,
    })
}
/// Kết quả kiểm tra: kết nối TCP + đọc FC03 thành công thì là "đã kết nối";
/// `mapping_warning` mô tả dữ liệu đọc được không khớp register map AS200.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeReport {
    pub mapping_warning: Option<String>,
}

/// `Err` chỉ khi không kết nối/đọc được PLC. Đọc được nhưng sai mapping → `Ok(Some(..))`.
async fn probe_link(config: SimulatorConfig) -> Result<Option<String>, String> {
    let checked = tokio::time::timeout(config.reader().timeout, async {
        let report = match As200Reader::new(config.reader())
            .poll_published_record()
            .await
        {
            Ok(report) => report,
            Err(e @ (As200Error::Transport(_) | As200Error::Timeout)) => {
                eprintln!("PLC settings FC03 probe: {e}");
                return Err(CONNECT.to_string());
            }
            // Đã đọc được thanh ghi; lỗi nằm ở giá trị/mapping.
            Err(e) => return Ok(Err(e.to_string())),
        };
        Ok(as200_structure(&report))
    })
    .await
    .map_err(|_| CONNECT.to_string())??;
    Ok(checked.err().inspect(|warning| {
        eprintln!("PLC settings FC03 probe: kết nối OK, sai mapping/dữ liệu: {warning}");
    }))
}
/// Dùng trước khi bật thu + ACK: bắt buộc mapping đúng, không chỉ kết nối được.
async fn probe(config: SimulatorConfig) -> Result<(), String> {
    match probe_link(config).await? {
        None => Ok(()),
        Some(warning) => Err(format!(
            "Đã kết nối và đọc được PLC nhưng dữ liệu không khớp register map AS200: {warning}"
        )),
    }
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
        if !report.diagnostics.is_empty() {
            return Err("AS200 trả buffer chưa ổn định hoặc có bản ghi không hợp lệ.".into());
        }
        Ok(())
    }
}

async fn prepare_managed_runtime(
    file: &Path,
    before: &str,
    config: &SimulatorConfig,
    endpoint: &Endpoint,
    pool: &sqlx::PgPool,
) -> Result<(String, SimulatorConfig), String> {
    if !simulator_config::ack_scope_allows(config) {
        return Err(format!(
            "Bản phát hành chưa được kỹ thuật phê duyệt cho phạm vi ACK AS200 ({}).",
            simulator_config::AS200_ACK_SCOPE_ID
        ));
    }
    let (source_key, session_id) = ensure_managed_source(pool, endpoint).await?;

    let mut next = config.clone();
    next.mode = "modbus".into();
    next.enabled = true;
    next.technical_setup_confirmed = true;
    next.allow_plc_connection = true;
    next.ack_enabled = true;
    next.ack_protocol = Some("as200".into());
    next.word_order = StoredWordOrder::Low;
    next.modbus_address_offset = 0;
    next.plc_heartbeat_timeout_ms = Some(5000);
    next.poll_interval_ms = 500;
    next.timeout_ms = 2000;
    next.source_key = source_key.clone();
    next.confirmed_session_id = Some(session_id);

    let journal = file
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("local-runtime")
        .join("managed-sources")
        .join(
            source_key
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>(),
        )
        .join("capture.json");
    next.journal_path = Some(journal.clone());
    next.validate()
        .map_err(|error| format!("Cấu hình runtime AS200 không hợp lệ: {error}"))?;

    if journal.exists() {
        let existing = std::fs::read_to_string(&journal)
            .map_err(|_| "Không thể đọc Journal của nguồn PLC đã xác minh.".to_string())?;
        if existing.trim().is_empty() {
            return Err("Journal của nguồn PLC bị rỗng; chưa thể ACK an toàn.".into());
        }
    } else {
        let history: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id WHERE s.source_key=$1) OR EXISTS(SELECT 1 FROM measurements m JOIN plc_sources s ON s.id=m.source_id WHERE s.source_key=$1)")
            .bind(&next.source_key).fetch_one(pool).await.map_err(|e| e.to_string())?;
        if history {
            return Err("Nguồn đã có lịch sử nhưng thiếu Journal v4; cần kỹ thuật đối chiếu, không tự tạo checkpoint.".into());
        }
        read_new_baseline(&next).await?;
        std::fs::create_dir_all(journal.parent().ok_or(CONFIG)?)
            .map_err(|_| "Không thể tạo thư mục runtime cho Journal PLC.".to_string())?;
        WorkerState::initialize_separate_journal(next.clone(), journal.clone()).await?;
    }

    let mut document = simulator_config::configuration_values(before)?;
    set_document_value(&mut document, "mode", toml::Value::String("modbus".into()))?;
    set_document_value(
        &mut document,
        "address",
        toml::Value::String(next.address.clone()),
    )?;
    set_document_value(
        &mut document,
        "unit_id",
        toml::Value::Integer(i64::from(next.unit_id)),
    )?;
    set_document_value(&mut document, "enabled", toml::Value::Boolean(true))?;
    set_document_value(
        &mut document,
        "technical_setup_confirmed",
        toml::Value::Boolean(true),
    )?;
    set_document_value(
        &mut document,
        "allow_plc_connection",
        toml::Value::Boolean(true),
    )?;
    set_document_value(&mut document, "ack_enabled", toml::Value::Boolean(true))?;
    set_document_value(
        &mut document,
        "ack_protocol",
        toml::Value::String("as200".into()),
    )?;
    set_document_value(
        &mut document,
        "word_order",
        toml::Value::String("LOW".into()),
    )?;
    set_document_value(
        &mut document,
        "modbus_address_offset",
        toml::Value::Integer(0),
    )?;
    set_document_value(
        &mut document,
        "plc_heartbeat_timeout_ms",
        toml::Value::Integer(5000),
    )?;
    set_document_value(&mut document, "poll_interval_ms", toml::Value::Integer(500))?;
    set_document_value(&mut document, "timeout_ms", toml::Value::Integer(2000))?;
    set_document_value(&mut document, "source_key", toml::Value::String(source_key))?;
    set_document_value(
        &mut document,
        "confirmed_session_id",
        toml::Value::String(next.confirmed_session_id.clone().ok_or(CONFIG)?),
    )?;
    set_document_value(
        &mut document,
        "journal_path",
        toml::Value::String(journal.to_string_lossy().into_owned()),
    )?;
    let after = toml::to_string(&document).map_err(|_| SAVE.to_string())?;
    simulator_config::parse(&after)
        .and_then(|parsed| parsed.validate())
        .map_err(|error| format!("Không thể tạo cấu hình runtime AS200: {error}"))?;
    Ok((after, next))
}
#[tauri::command]
pub async fn stop_plc_collection(state: tauri::State<'_, WorkerState>) -> Result<(), String> {
    let _guard = state.settings_gate.lock().await;
    state.stop_collection().await
}

pub(super) fn clean_new_baseline(
    report: &super::as200::reader::PublishedRecordSnapshot,
) -> Result<(), String> {
    let c = &report.control;
    if c.total_count != 0
        || c.pc_read_count != 0
        || c.pc_read_done != 0
        || c.data_ready != 0
        || !report.diagnostics.is_empty()
        || report.buffer.is_some()
    {
        return Err("Chưa thể xác minh phiên đo mới. Máy đo cần ở trạng thái khởi tạo sạch, chưa có kết quả đo. Vui lòng liên hệ kỹ thuật.".into());
    }
    Ok(())
}

pub(super) async fn read_new_baseline(config: &SimulatorConfig) -> Result<(), String> {
    use super::as200::reader::PublishedRecordSnapshot;
    let mut reader = As200Reader::new(config.reader());
    let before = reader.read_control().await.map_err(|e| e.to_string())?;
    clean_new_baseline(&PublishedRecordSnapshot {
        control: before.clone(),
        buffer: None,
        diagnostics: Vec::new(),
    })?;
    let raw = reader
        .read_published_words()
        .await
        .map_err(|e| e.to_string())?;
    let middle = reader.read_control().await.map_err(|e| e.to_string())?;
    let confirm = reader
        .read_published_words()
        .await
        .map_err(|e| e.to_string())?;
    let after = reader.read_control().await.map_err(|e| e.to_string())?;
    for control in [middle, after] {
        clean_new_baseline(&PublishedRecordSnapshot {
            control: control.clone(),
            buffer: None,
            diagnostics: Vec::new(),
        })?;
        if !before.same_buffer_state(&control) || before.reserve != control.reserve {
            return Err("Baseline máy đo thay đổi khi xác minh; chưa thể tạo phiên mới.".into());
        }
    }
    if raw != confirm || raw.len() != map::RECORD_SIZE || raw.iter().any(|word| *word != 0) {
        return Err("Vùng Bản ghi 1 chưa rỗng và ổn định; chưa thể tạo phiên mới.".into());
    }
    Ok(())
}

struct RecordedProfile {
    text: String,
    config: SimulatorConfig,
}

const RESET_CONFIRMATION_REQUIRED: &str = "RESET_CONFIRMATION_REQUIRED";

// The caller holds the closed-source journal lock. This creates a separate journal;
// neither the old checkpoint nor its receipts are rewritten.
async fn prepare_confirmed_reset(
    file: &Path,
    previous_text: &str,
    previous: &SimulatorConfig,
    pool: &sqlx::PgPool,
    confirmed: bool,
) -> Result<RecordedProfile, String> {
    if previous.pc_heartbeat_enabled || !file.with_extension("operator-stop").exists() {
        return Err("Chưa thể bắt đầu phiên mới: cần dừng thu và tắt PC Heartbeat ghi.".into());
    }
    read_new_baseline(previous).await?;
    if !confirmed {
        return Err(RESET_CONFIRMATION_REQUIRED.into());
    }
    let session = format!(
        "qc-reset-{}-{}",
        chrono::Utc::now().timestamp_nanos_opt().ok_or(SAVE)?,
        std::process::id()
    );
    let used: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id WHERE s.source_key=$1 AND i.session_id=$2) OR EXISTS(SELECT 1 FROM measurements m JOIN plc_sources s ON s.id=m.source_id WHERE s.source_key=$1 AND m.session_id=$2)")
        .bind(&previous.source_key).bind(&session).fetch_one(pool).await
        .map_err(|_| "Không thể đối chiếu phiên mới trong cơ sở dữ liệu.".to_string())?;
    if used {
        return Err("Phiên mới đã có lịch sử; chưa thể tiếp tục.".into());
    }
    let directory = file
        .parent()
        .ok_or(CONFIG)?
        .join("local-runtime/managed-sources")
        .join(&session);
    std::fs::create_dir_all(directory.parent().ok_or(SAVE)?).map_err(|_| SAVE.to_string())?;
    std::fs::create_dir(&directory).map_err(|_| SAVE.to_string())?;
    let mut next = previous.clone();
    next.confirmed_session_id = Some(session);
    next.journal_path = Some(directory.join("capture.json"));
    next.session_transition = None;
    next.validate().map_err(|_| CONFIG.to_string())?;
    let mut document = simulator_config::configuration_values(previous_text)?;
    set_document_value(
        &mut document,
        "confirmed_session_id",
        toml::Value::String(next.confirmed_session_id.clone().ok_or(CONFIG)?),
    )?;
    set_document_value(
        &mut document,
        "journal_path",
        toml::Value::String(
            next.journal_path
                .as_ref()
                .ok_or(CONFIG)?
                .to_string_lossy()
                .into_owned(),
        ),
    )?;
    let serialized = toml::to_string(&document).map_err(|_| SAVE.to_string())?;
    let text = format!(
        "# Người phụ trách xác nhận đúng nguồn vật lý, PLC đã reset và không còn phép đo bị bỏ sót lúc {}.\n{}",
        chrono::Utc::now(),
        serialized
    );
    for (name, contents) in [
        ("previous-config.toml", previous_text),
        ("target-config.toml", text.as_str()),
    ] {
        let mut output = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(directory.join(name))
            .map_err(|_| SAVE.to_string())?;
        output
            .write_all(contents.as_bytes())
            .and_then(|_| output.sync_all())
            .map_err(|_| SAVE.to_string())?;
    }
    WorkerState::initialize_separate_journal(
        next.clone(),
        next.journal_path.clone().ok_or(CONFIG)?,
    )
    .await?;
    read_new_baseline(&next).await?;
    Ok(RecordedProfile { text, config: next })
}

fn endpoint_link(file: &Path, value: &Endpoint) -> Result<PathBuf, String> {
    let ip: IpAddr = value.ip.parse().map_err(|_| CONFIG.to_string())?;
    Ok(file
        .parent()
        .ok_or(CONFIG)?
        .join("local-runtime")
        .join("endpoint-links")
        .join(format!(
            "{}-{}-{}.toml",
            ip.to_string().replace(':', "_"),
            value.port,
            value.unit_id
        )))
}

fn remember_endpoint(file: &Path, text: &str, config: &SimulatorConfig) -> Result<(), String> {
    let path = endpoint_link(file, &endpoint(config)?)?;
    std::fs::create_dir_all(path.parent().ok_or(SAVE)?).map_err(|_| SAVE.to_string())?;
    let mut document: toml::Value = simulator_config::configuration_values(text)?;
    set_document_value(
        &mut document,
        "journal_path",
        toml::Value::String(
            profile_journal(file, config)?
                .to_string_lossy()
                .into_owned(),
        ),
    )?;
    let after = toml::to_string(&document).map_err(|_| SAVE.to_string())?;
    if path.exists() {
        let before = std::fs::read_to_string(&path).map_err(|_| SAVE.to_string())?;
        replace(&path, &before, &after)
    } else {
        let temp = path.with_extension(format!(
            "{}-{}.tmp",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().ok_or(SAVE)?
        ));
        let mut output = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
            .map_err(|_| SAVE.to_string())?;
        output
            .write_all(after.as_bytes())
            .and_then(|_| output.sync_all())
            .map_err(|_| SAVE.to_string())?;
        drop(output);
        std::fs::rename(temp, path).map_err(|_| SAVE.to_string())
    }
}

fn retain_current_profile(file: &Path, text: &str, config: &SimulatorConfig) -> Result<(), String> {
    let managed = file
        .parent()
        .ok_or(CONFIG)?
        .join("local-runtime")
        .join("managed-sources");
    if !managed.exists() {
        return Ok(());
    }
    let journal = profile_journal(file, config)?;
    let directory = journal.parent().ok_or(CONFIG)?;
    if directory.parent()
        != Some(
            managed
                .canonicalize()
                .map_err(|_| CONFIG.to_string())?
                .as_path(),
        )
    {
        return Ok(()); // Legacy external journals remain described by previous-config backups.
    }
    let target = directory.join("target-config.toml");
    if !target.exists() {
        // Preserve provenance before switching away from a legacy profile; never rewrite an existing record.
        let mut saved = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(target)
            .map_err(|_| SAVE.to_string())?;
        saved
            .write_all(text.as_bytes())
            .and_then(|_| saved.sync_all())
            .map_err(|_| SAVE.to_string())?;
    }
    Ok(())
}

fn profile_journal(file: &Path, config: &SimulatorConfig) -> Result<PathBuf, String> {
    config.journal_path.as_ref().ok_or(CONFIG)?;
    let p = config.journal_file(file);
    let journal = p
        .parent()
        .ok_or(CONFIG)?
        .canonicalize()
        .map_err(|_| CONFIG.to_string())?
        .join(p.file_name().ok_or(CONFIG)?);
    let runtime = file
        .parent()
        .ok_or(CONFIG)?
        .join("local-runtime")
        .canonicalize()
        .map_err(|_| CONFIG.to_string())?;
    if !journal.starts_with(runtime) {
        return Err("Journal phải nằm trong local-runtime cạnh ứng dụng; cần kỹ thuật đối chiếu lịch sử cũ.".into());
    }
    Ok(journal)
}

fn previous_endpoint(file: &Path, requested: &Endpoint) -> Result<Option<RecordedProfile>, String> {
    let root = file
        .parent()
        .ok_or(CONFIG)?
        .join("local-runtime")
        .join("managed-sources");
    if !root.exists() {
        return if endpoint_link(file, requested)?.exists() {
            Err("HISTORY_INCOMPLETE".into())
        } else {
            Ok(None)
        };
    }
    let unknown = "HISTORY_INCOMPLETE";
    let (text, config) = read(file)?;
    let mut records = vec![RecordedProfile { text, config }];
    let link = endpoint_link(file, requested)?;
    let remembered = if link.exists() {
        let (text, mut config) = read(&link).map_err(|_| unknown.to_string())?;
        if endpoint(&config)? != *requested {
            return Err(unknown.into());
        }
        config.journal_path =
            Some(profile_journal(file, &config).map_err(|_| unknown.to_string())?);
        Some(RecordedProfile { text, config })
    } else {
        None
    };
    let mut directories = Vec::new();
    for entry in std::fs::read_dir(&root).map_err(|_| unknown.to_string())? {
        let dir = entry.map_err(|_| unknown.to_string())?.path();
        if !dir.is_dir() {
            return Err(unknown.into());
        }
        directories.push(dir.clone());
        for name in ["previous-config.toml", "target-config.toml"] {
            let path = dir.join(name);
            if path.exists() {
                let (text, config) = read(&path).map_err(|_| unknown.to_string())?;
                records.push(RecordedProfile { text, config });
            }
        }
    }
    // A later previous-config is also authoritative provenance for a legacy profile without target-config.
    for dir in directories {
        let journal = dir
            .canonicalize()
            .map_err(|_| unknown.to_string())?
            .join("capture.json");
        if !records
            .iter()
            .any(|r| profile_journal(file, &r.config).ok().as_ref() == Some(&journal))
        {
            return Err(unknown.into());
        }
    }
    if let Some(remembered) = remembered {
        if !records.iter().any(|r| {
            endpoint(&r.config).ok().as_ref() == Some(requested)
                && r.config.source_key == remembered.config.source_key
                && r.config.confirmed_session_id == remembered.config.confirmed_session_id
                && r.config.word_order.as_str() == remembered.config.word_order.as_str()
                && r.config.modbus_address_offset == remembered.config.modbus_address_offset
                && profile_journal(file, &r.config).ok() == remembered.config.journal_path
        }) {
            return Err(unknown.into());
        }
        // This pointer records a completed/closed application association, not hardware identity.
        // The caller must still verify journal/DB and the live snapshot before resuming.
        return Ok(Some(remembered));
    }
    let mut found: Option<RecordedProfile> = None;
    for mut record in records {
        if endpoint(&record.config)? != *requested {
            continue;
        }
        let journal = profile_journal(file, &record.config).map_err(|_| unknown.to_string())?;
        record.config.journal_path = Some(journal.clone());
        if let Some(previous) = &found {
            if previous.config.source_key != record.config.source_key
                || previous.config.confirmed_session_id != record.config.confirmed_session_id
                || previous.config.journal_path != Some(journal)
                || previous.config.word_order.as_str() != record.config.word_order.as_str()
                || previous.config.modbus_address_offset != record.config.modbus_address_offset
            {
                return Err("HISTORY_AMBIGUOUS".into());
            }
        } else {
            found = Some(record);
        }
    }
    Ok(found)
}

#[tauri::command]
pub async fn connect_plc_collection(
    value: Endpoint,
    expected_revision: String,
    reset_confirmed: Option<bool>,
    state: tauri::State<'_, WorkerState>,
    database: tauri::State<'_, crate::database::DatabaseState>,
) -> Result<(), String> {
    let trace = format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    switch_log(
        &trace,
        "SWITCH_START",
        "OBSERVED",
        format!(
            "target_endpoint={}:{} unit={} reset_confirmed={}",
            value.ip,
            value.port,
            value.unit_id,
            reset_confirmed == Some(true)
        ),
    );
    let mut stage = "SWITCH_START";
    let result: Result<(), String> = async {
    let _guard = state.settings_gate.lock().await;
    stage = "WORKER_STOP_CHECK";
    switch_log(&trace, stage, "OBSERVED", format!("worker_enabled={}", state.status.read().await.enabled));
    if state.settings_path.lock().await.is_none() {
        return Err("Chưa lưu cấu hình PLC. Hãy lưu địa chỉ IP, cổng và Unit ID trước; kỹ thuật cần xác nhận thông số trước khi bật thu dữ liệu.".into());
    }
    let file = path(&state).await?;
    let (text, config) = read(&file)?;
    if file.with_extension("connection-pending").exists() {
        return Err("Chuyển kết nối trước chưa hoàn tất. Vui lòng liên hệ kỹ thuật để đối chiếu cấu hình.".into());
    }
    if revision(&text) != expected_revision {
        return Err("Thông tin kết nối đã thay đổi. Vui lòng tải lại trang.".into());
    }
    let mut next = value.apply(&config)?;
    let mut changed = endpoint(&next)? != endpoint(&config)?;
    switch_log(&trace, "SOURCE_RESOLVED", "OBSERVED", format!("changed={changed} target_endpoint={}:{} unit={}", value.ip, value.port, value.unit_id));
    let status = state.status.read().await;
    let reset_recovery = !changed
        && status.enabled
        && status.pause_reason.as_deref() == Some("RESET");
    drop(status);
    if reset_recovery {
        stage = "WORKER_STOP_CHECK";
        state.stop_collection().await?;
        switch_log(
            &trace,
            stage,
            "PASS",
            "reset_paused_worker_stopped=true journal_flush_completed=true",
        );
    }
    stage = "PLC_PROBE";
    let mut probe_config = next.clone();
    probe_config.mode = "modbus".into();
    probe_config.word_order = StoredWordOrder::Low;
    probe_config.modbus_address_offset = 0;
    probe_config.poll_interval_ms = 500;
    probe_config.timeout_ms = 2000;
    probe_config.plc_heartbeat_timeout_ms = Some(5000);
    probe(probe_config.clone()).await?;
    switch_log(&trace, "PLC_PROBE", "PASS", "fc03=true as200_structure=true control_and_buffers_valid=true");
    let pool = database.pool().await?;
    if !simulator_config::ack_scope_allows(&probe_config) {
        return Err(format!(
            "Bản phát hành chưa được kỹ thuật phê duyệt cho phạm vi ACK AS200 ({}).",
            simulator_config::AS200_ACK_SCOPE_ID
        ));
    }
    let worker_running = state.status.read().await.enabled;
    if worker_running {
        if !changed {
            return Err("Worker đang thu từ PLC này; không cần chuyển kết nối.".into());
        }
        stage = "WORKER_STOP_CHECK";
        state.stop_collection().await?;
        switch_log(&trace, stage, "PASS", "old_worker_stopped=true journal_flush_completed=true");
    }
    let mut replacement = None;
    let mut runtime_hydrated = false;
    if !config.technical_setup_confirmed {
        let (after, hydrated) =
            prepare_managed_runtime(&file, &text, &next, &value, &pool).await?;
        next = hydrated;
        replacement = Some(after);
        changed = false;
        runtime_hydrated = true;
        switch_log(&trace, "RUNTIME_CONFIGURED", "PASS", "managed_as200_source=true defaults_applied=true");
    } else {
        config.validate().map_err(|error| format!("Cấu hình thu dữ liệu chưa hợp lệ: {error}"))?;
        if !config.enabled {
            return Err("Cấu hình PLC chưa bật thu dữ liệu.".into());
        }
    }
    let mut old_source_lock = None;
    if changed {
        stage = "SOURCE_RESOLVED";
        let history = previous_endpoint(&file, &value).map_err(|e| match e.as_str() {
            "HISTORY_AMBIGUOUS" => "Địa chỉ này có nhiều lịch sử nguồn đo. Chưa thể chọn phiên an toàn; vui lòng liên hệ kỹ thuật.".to_string(),
            "HISTORY_INCOMPLETE" => "Thiếu cấu hình đối chiếu của một nguồn đã lưu. Chưa thể xác minh lịch sử địa chỉ máy đo.".to_string(),
            _ => e,
        })?;
        switch_log(&trace, "SOURCE_RESOLVED", if history.is_some() { "HISTORICAL_PROFILE" } else { "NEW_PROFILE" }, format!("endpoint={}:{} unit={} historical={}", value.ip, value.port, value.unit_id, history.is_some()));
        if history.is_none() {
            let _old_source_lock =
                state.verify_source_closed(&file, &config, &pool, true, &trace).await?;
            let (after, hydrated) =
                prepare_managed_runtime(&file, &text, &next, &value, &pool).await?;
            next = hydrated;
            replacement = Some(after);
            changed = false;
            switch_log(&trace, "RUNTIME_CONFIGURED", "PASS", "new_managed_source=true defaults_applied=true");
        }
        if let Some(profile) = history {
            stage = "JOURNAL_CHECK";
            old_source_lock = Some(state.verify_source_closed(&file, &config, &pool, true, &trace).await?);
            next = profile.config;
            if !next.enabled || next.pc_heartbeat_enabled {
                switch_log(&trace, "SOURCE_RESOLVED", "SOURCE_POLICY_MISMATCH", format!("expected_enabled=true actual_enabled={} expected_pc_heartbeat=false actual_pc_heartbeat={}", next.enabled, next.pc_heartbeat_enabled));
                return Err("Nguồn đo cũ chưa đủ điều kiện kết nối lại an toàn.".into());
            }
            next.validate().map_err(|_| CONFIG.to_string())?;
            let historical_lock = state.verify_source_closed(&file, &next, &pool, true, &trace).await?;
            stage = "PLC_SNAPSHOT";
            if state.verify_resume(&file, &next, true, &database, &trace).await? {
                let reset = prepare_confirmed_reset(&file, &profile.text, &next, &pool, reset_confirmed == Some(true)).await?;
                next = reset.config;
                replacement = Some(reset.text);
            } else {
                let mut document = simulator_config::configuration_values(&profile.text)?;
                set_document_value(
                    &mut document,
                    "journal_path",
                    toml::Value::String(next.journal_path.as_ref().ok_or(CONFIG)?.to_string_lossy().into_owned()),
                )?;
                replacement = Some(toml::to_string(&document).map_err(|_| SAVE.to_string())?);
            }
            drop(historical_lock);
        }
    } else {
        stage = "PLC_SNAPSHOT";
        if !runtime_hydrated && state.verify_resume(&file, &config, false, &database, &trace).await? {
            let pool = database.pool().await?;
            old_source_lock = Some(state.verify_source_closed(&file, &config, &pool, true, &trace).await?);
            let reset = prepare_confirmed_reset(&file, &text, &config, &pool, reset_confirmed == Some(true)).await?;
            next = reset.config;
            replacement = Some(reset.text);
        }
    }
    let marker = file.with_extension("operator-stop");
    stage = "WORKER_STOP_CHECK";
    if changed && !marker.exists() {
        return Err("Vui lòng dừng thu dữ liệu trước khi chuyển kết nối.".into());
    }
    let pending = file.with_extension("connection-pending");
    stage = "CONFIG_SAVE";
    if let Some(after) = &replacement {
        if config.journal_path.is_some() && !config.source_key.trim().is_empty() {
            retain_current_profile(&file, &text, &config)?;
            // The outgoing profile has passed closed-source checks. Keep its association for a later return.
            remember_endpoint(&file, &text, &config)?;
        }
        let mut record = std::fs::OpenOptions::new().create_new(true).write(true).open(&pending).map_err(|_| SAVE.to_string())?;
        record.write_all(text.as_bytes()).and_then(|_| record.sync_all()).map_err(|_| SAVE.to_string())?;
        if let Err(error) = replace(&file, &text, after) {
            eprintln!("PLC settings replace failed: {error}");
            let _ = std::fs::remove_file(&pending);
            return Err(SAVE.into());
        }
    }
    if marker.exists() {
        if let Err(error) = std::fs::remove_file(&marker) {
            eprintln!("PLC stop marker removal failed: {error}");
            if let Some(after) = &replacement {
                if replace(&file, after, &text).is_ok() { let _ = std::fs::remove_file(&pending); }
            }
            return Err(SAVE.into());
        }
    }
    let dir = state.settings_directory.lock().await.clone();
    stage = "WORKER_START";
    state.inner().clone().launch_under_gate(dir, database.inner().clone()).await;
    let status = state.status.read().await;
    let failed = status.state == "ERROR" || !status.enabled;
    let start_error = status.message.clone();
    drop(status);
    if failed {
        state.stop_collection().await?;
        if let Some(after) = &replacement {
            replace(&file, after, &text).map_err(|_| "Không thể khôi phục cấu hình cũ; hệ thống đã giữ trạng thái dừng. Vui lòng liên hệ kỹ thuật.".to_string())?;
            let _ = std::fs::remove_file(&pending);
        }
        return Err(format!("Chưa thể khởi động thu dữ liệu: {start_error}"));
    }
    let finalization = if let Some(after) = &replacement {
        remember_endpoint(&file, after, &next).and_then(|_| std::fs::remove_file(&pending).map_err(|_| SAVE.to_string()))
    } else { Ok(()) };
    if finalization.is_err() {
        state.stop_collection().await?;
        if let Some(after) = &replacement {
            replace(&file, after, &text).map_err(|_| "Chưa thể khôi phục cấu hình; hệ thống đã dừng và giữ dấu đối chiếu. Vui lòng liên hệ kỹ thuật.".to_string())?;
        }
        return Err("Không thể hoàn tất chuyển kết nối; đã dừng thu dữ liệu. Vui lòng liên hệ kỹ thuật.".into());
    }
    state.settings_restart.store(false, Ordering::SeqCst);
    drop(old_source_lock);
    Ok(())
    }.await;
    match &result {
        Ok(()) => switch_log(
            &trace,
            "SWITCH_ALLOWED",
            "PASS",
            "configuration_committed=true worker_started=true",
        ),
        Err(error) => switch_log(
            &trace,
            "SWITCH_BLOCKED",
            "GUARD_OR_OPERATION_FAILED",
            format!("failed_step={stage} error={error}"),
        ),
    }
    result
}

#[tauri::command]
pub async fn get_plc_connection_settings(
    state: tauri::State<'_, WorkerState>,
) -> Result<Settings, String> {
    let _guard = state.settings_gate.lock().await;
    get(&state).await
}
#[tauri::command]
pub async fn test_plc_connection(
    value: Endpoint,
    state: tauri::State<'_, WorkerState>,
) -> Result<ProbeReport, String> {
    let _guard = state
        .settings_gate
        .try_lock()
        .map_err(|_| "Đang thực hiện thao tác kết nối. Vui lòng chờ.".to_string())?;
    let config = if state.settings_path.lock().await.is_none() {
        if !state.settings_ready.load(Ordering::SeqCst) {
            return Err(CONFIG.into());
        }
        draft(&value)?
    } else {
        read(&path(&state).await?)?.1
    };
    let mapping_warning = probe_link(value.apply(&config)?).await?;
    Ok(ProbeReport { mapping_warning })
}
#[tauri::command]
pub async fn update_plc_connection_settings(
    value: Endpoint,
    expected_revision: String,
    state: tauri::State<'_, WorkerState>,
) -> Result<Settings, String> {
    let _guard = state
        .settings_gate
        .try_lock()
        .map_err(|_| "Đang thực hiện thao tác kết nối. Vui lòng chờ.".to_string())?;
    save(&state, value, &expected_revision).await
}

#[tauri::command]
pub async fn confirm_initial_plc_setup(
    value: InitialTechnicalSetup,
    expected_revision: String,
    state: tauri::State<'_, WorkerState>,
    database: tauri::State<'_, crate::database::DatabaseState>,
) -> Result<Settings, String> {
    let _guard = state.settings_gate.lock().await;
    let file = path(&state).await?;
    let (before, config) = read(&file)?;
    if revision(&before) != expected_revision {
        return Err("Cấu hình đã thay đổi. Vui lòng tải lại trang trước khi xác nhận.".into());
    }
    if !value.new_source_confirmed || !value.register_map_confirmed || !value.ack_confirmed {
        return Err(
            "Cần xác nhận nguồn PLC mới, thông số AS200 và quyền ACK trước khi bật thu.".into(),
        );
    }
    if config.mode != "modbus"
        || config.enabled
        || config.technical_setup_confirmed
        || !config.source_key.is_empty()
        || config.confirmed_session_id.is_some()
        || config.journal_path.is_some()
        || config.ack_enabled
        || config.pc_heartbeat_enabled
        || state.status.read().await.enabled
        || file.with_extension("connection-pending").exists()
        || file.with_extension("operator-stop").exists()
        || file.parent().ok_or(CONFIG)?.join("local-runtime").exists()
        || file
            .parent()
            .ok_or(CONFIG)?
            .join("as200_capture_journal.json")
            .exists()
        || file
            .parent()
            .ok_or(CONFIG)?
            .join("as200_capture_journal.lock")
            .exists()
    {
        return Err("Cấu hình hoặc lịch sử nguồn đo đã tồn tại; cần kỹ thuật đối chiếu trước khi xác nhận nguồn mới.".into());
    }
    let _safe = state
        .ensure_connection_edit_safe(&file, &config)
        .await
        .map_err(|_| "Worker chưa dừng an toàn hoặc còn lịch sử cần đối chiếu.".to_string())?;
    let mut next = config.clone();
    next.word_order = value.word_order;
    next.modbus_address_offset = value.modbus_address_offset;
    next.plc_heartbeat_timeout_ms = Some(value.plc_heartbeat_timeout_ms);
    next.ack_protocol = Some("as200".into());
    next.ack_enabled = true;
    next.allow_plc_connection = true;
    next.technical_setup_confirmed = true;
    next.enabled = true;
    next.validate_read()?;
    if !(2000..=3600000).contains(&value.plc_heartbeat_timeout_ms) {
        return Err("Ngưỡng giám sát heartbeat phải từ 2000 đến 3600000 ms.".into());
    }
    read_new_baseline(&next).await?;
    let pool = database.pool().await?;
    let id = format!(
        "qc-{}-{}",
        chrono::Utc::now().timestamp_nanos_opt().ok_or(SAVE)?,
        std::process::id()
    );
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM plc_sources WHERE source_key=$1)")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .map_err(|_| "Không thể kiểm tra nguồn dữ liệu mới.".to_string())?;
    if exists {
        return Err("Mã nguồn mới đã tồn tại; vui lòng thử lại.".into());
    }
    let directory = file
        .parent()
        .ok_or(SAVE)?
        .join("local-runtime")
        .join("managed-sources")
        .join(&id);
    std::fs::create_dir_all(directory.parent().ok_or(SAVE)?).map_err(|_| SAVE.to_string())?;
    std::fs::create_dir(&directory).map_err(|_| SAVE.to_string())?;
    let journal = directory.join("capture.json");
    next.source_key = id.clone();
    next.confirmed_session_id = Some(format!("{id}-initial"));
    next.journal_path = Some(journal.clone());
    next.validate()?;
    let mut document = simulator_config::configuration_values(&before)?;
    set_document_value(
        &mut document,
        "word_order",
        toml::Value::String(next.word_order.as_str().into()),
    )?;
    set_document_value(
        &mut document,
        "modbus_address_offset",
        toml::Value::Integer(next.modbus_address_offset.into()),
    )?;
    set_document_value(
        &mut document,
        "plc_heartbeat_timeout_ms",
        toml::Value::Integer(
            value
                .plc_heartbeat_timeout_ms
                .try_into()
                .map_err(|_| CONFIG)?,
        ),
    )?;
    set_document_value(
        &mut document,
        "ack_protocol",
        toml::Value::String("as200".into()),
    )?;
    set_document_value(&mut document, "ack_enabled", toml::Value::Boolean(true))?;
    set_document_value(
        &mut document,
        "allow_plc_connection",
        toml::Value::Boolean(true),
    )?;
    set_document_value(
        &mut document,
        "technical_setup_confirmed",
        toml::Value::Boolean(true),
    )?;
    set_document_value(&mut document, "enabled", toml::Value::Boolean(true))?;
    set_document_value(
        &mut document,
        "source_key",
        toml::Value::String(next.source_key.clone()),
    )?;
    set_document_value(
        &mut document,
        "confirmed_session_id",
        toml::Value::String(next.confirmed_session_id.clone().ok_or(CONFIG)?),
    )?;
    set_document_value(
        &mut document,
        "journal_path",
        toml::Value::String(journal.to_string_lossy().into_owned()),
    )?;
    let after = toml::to_string(&document).map_err(|_| SAVE.to_string())?;
    simulator_config::parse(&after).and_then(|c| c.validate())?;
    // Endpoint-only setup has no source history; only the confirmed profile is reusable.
    let mut output = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(directory.join("target-config.toml"))
        .map_err(|_| SAVE.to_string())?;
    output
        .write_all(after.as_bytes())
        .and_then(|_| output.sync_all())
        .map_err(|_| SAVE.to_string())?;
    WorkerState::initialize_separate_journal(next.clone(), journal).await?;
    read_new_baseline(&next).await?;
    replace(&file, &before, &after).map_err(|_| "Đã giữ hồ sơ nguồn mới nhưng chưa cập nhật được cấu hình; cần kỹ thuật đối chiếu trước khi thử lại.".to_string())?;
    state.settings_restart.store(true, Ordering::SeqCst);
    get(&state).await
}

#[cfg(test)]
mod tests;

fn draft(value: &Endpoint) -> Result<SimulatorConfig, String> {
    value.apply(&SimulatorConfig {
        mode: "modbus".into(),
        ..Default::default()
    })
}
fn create_initial(dir: &Path, value: &Endpoint) -> Result<PathBuf, String> {
    let config = draft(value)?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join("as200_config.toml");
    // A missing config beside an existing journal is recovery, not a new installation.
    if dir.join("as200_capture_journal.json").exists()
        || dir.join("as200_capture_journal.lock").exists()
    {
        return Err("Existing local history requires technical reconciliation".into());
    }
    let text = format!(
        "# Endpoint only. Technical commissioning is required before enabling collection.\nmode = \"modbus\"\nenabled = false\ntechnical_setup_confirmed = false\naddress = \"{}\"\nunit_id = {}\nack_protocol = \"as200\"\nword_order = \"LOW\"\nmodbus_address_offset = 0\npoll_interval_ms = 500\ntimeout_ms = 2000\nplc_heartbeat_timeout_ms = 5000\nack_enabled = false\npc_heartbeat_enabled = false\nallow_plc_connection = false\nallow_plc_heartbeat_write = false\n",
        config.address, config.unit_id
    );
    simulator_config::parse(&text)?.validate_read()?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("settings.lock"))
        .map_err(|e| e.to_string())?;
    lock.try_lock().map_err(|e| e.to_string())?;
    if path.exists() {
        return Err("Configuration already exists".into());
    }
    let temporary = dir.join(format!(
        ".initial-plc-{}-{}.tmp",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        // Link fails if the destination appeared meanwhile, so never overwrite an existing config.
        std::fs::hard_link(&temporary, &path).map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_file(&temporary);
    result?;
    Ok(path)
}
