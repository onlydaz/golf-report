//! Controlled QC connection lifecycle with journal/inbox and source continuity checks.
use super::{
    as200::{
        map,
        reader::{HoldingRegisterReader, ModbusTcpReader},
    },
    simulator_config::{self, SimulatorConfig},
    worker::{WorkerState, switch_log},
};
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
    let resolved = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(dir) = std::env::var_os(key) {
            let root = PathBuf::from(dir).to_string_lossy().to_lowercase();
            if resolved
                .to_string_lossy()
                .to_lowercase()
                .starts_with(&(root + "\\"))
            {
                return Err(
                    "Configuration in Program Files requires technical provisioning in AppData"
                        .into(),
                );
            }
        }
    }
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
        if let Ok(cwd)=std::env::current_dir() {
            let mut folders=vec![cwd.clone(),cwd.join("src-tauri")];
            if cwd.file_name().is_some_and(|n|n=="src-tauri") {if let Some(parent)=cwd.parent(){folders.push(parent.to_path_buf());}}
            if folders.iter().any(|folder|folder.join("as200_capture_journal.json").exists() || folder.join("as200_capture_journal.lock").exists()) {
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
async fn probe(config: SimulatorConfig) -> Result<(), String> {
    let timeout = config.reader().timeout;
    tokio::time::timeout(timeout, async {
        let address = map::modbus_address(map::CONTROL_BASE, config.modbus_address_offset)
            .ok_or("probe address overflow".to_string())?;
        let words = ModbusTcpReader::new(config.reader())
            .read_holding_registers(address, 3)
            .await
            .map_err(|e| e.to_string())?;
        if words != [1, map::RECORD_SIZE as u16, map::BUFFER_COUNT as u16] {
            return Err("AS200 Control metadata does not match".into());
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|_| CONNECT.to_string())?
    .map_err(|e| {
        eprintln!("PLC settings FC03 probe: {e}");
        CONNECT.to_string()
    })?;
    Ok(())
}
#[tauri::command]
pub async fn stop_plc_collection(state: tauri::State<'_, WorkerState>) -> Result<(), String> {
    let _guard = state.settings_gate.lock().await;
    state.stop_collection().await
}

pub(super) fn clean_new_baseline(report: &super::as200::reader::PollReport) -> Result<(), String> {
    let c = &report.control;
    if c.total_count != 0 || c.pc_read_count != 0 || c.pc_read_done != 0 || c.data_ready != 0
        || !report.diagnostics.is_empty() || report.buffers.len() != 2
        || report.buffers.iter().any(|b| b.record.is_some() || b.raw_registers.len() != 240 || b.raw_registers.iter().any(|w| *w != 0)) {
        return Err("Chưa thể xác minh phiên đo mới. Máy đo cần ở trạng thái khởi tạo sạch, chưa có kết quả đo. Vui lòng liên hệ kỹ thuật.".into());
    }
    Ok(())
}

async fn read_new_baseline(config: &SimulatorConfig) -> Result<(), String> {
    let report = super::as200::reader::As200Reader::new(config.reader()).poll().await
        .map_err(|_| "Không thể kiểm tra dữ liệu máy đo mới.".to_string())?;
    clean_new_baseline(&report)
}

struct RecordedProfile { text: String, config: SimulatorConfig }

const RESET_CONFIRMATION_REQUIRED: &str = "RESET_CONFIRMATION_REQUIRED";

// The caller holds the closed-source journal lock. This creates a separate journal;
// neither the old checkpoint nor its receipts are rewritten.
async fn prepare_confirmed_reset(
    file: &Path, previous_text: &str, previous: &SimulatorConfig, pool: &sqlx::PgPool,
    confirmed: bool,
) -> Result<RecordedProfile, String> {
    if previous.pc_heartbeat_enabled
        || !file.with_extension("operator-stop").exists() {
        return Err("Chưa thể bắt đầu phiên mới: cần dừng thu và tắt PC Heartbeat ghi.".into());
    }
    read_new_baseline(previous).await?;
    if !confirmed {
        return Err(RESET_CONFIRMATION_REQUIRED.into());
    }
    let session = format!("qc-reset-{}-{}", chrono::Utc::now().timestamp_nanos_opt().ok_or(SAVE)?, std::process::id());
    let used: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id WHERE s.source_key=$1 AND i.session_id=$2) OR EXISTS(SELECT 1 FROM measurements m JOIN plc_sources s ON s.id=m.source_id WHERE s.source_key=$1 AND m.session_id=$2)")
        .bind(&previous.source_key).bind(&session).fetch_one(pool).await
        .map_err(|_| "Không thể đối chiếu phiên mới trong cơ sở dữ liệu.".to_string())?;
    if used { return Err("Phiên mới đã có lịch sử; chưa thể tiếp tục.".into()); }
    let directory = file.parent().ok_or(CONFIG)?.join("local-runtime/managed-sources").join(&session);
    std::fs::create_dir_all(directory.parent().ok_or(SAVE)?).map_err(|_| SAVE.to_string())?;
    std::fs::create_dir(&directory).map_err(|_| SAVE.to_string())?;
    let mut next = previous.clone();
    next.confirmed_session_id = Some(session);
    next.journal_path = Some(directory.join("capture.json"));
    next.session_transition = None;
    next.validate().map_err(|_| CONFIG.to_string())?;
    let mut document = simulator_config::configuration_values(previous_text)?;
    document["confirmed_session_id"] = toml::Value::String(next.confirmed_session_id.clone().ok_or(CONFIG)?);
    document["journal_path"] = toml::Value::String(next.journal_path.as_ref().ok_or(CONFIG)?.to_string_lossy().into_owned());
    let serialized = toml::to_string(&document).map_err(|_| SAVE.to_string())?;
    let text = format!("# Người phụ trách xác nhận đúng nguồn vật lý, PLC đã reset và không còn phép đo bị bỏ sót lúc {}.\n{}", chrono::Utc::now(), serialized);
    for (name, contents) in [
        ("previous-config.toml", previous_text), ("target-config.toml", text.as_str()),
    ] {
        let mut output = std::fs::OpenOptions::new().create_new(true).write(true)
            .open(directory.join(name)).map_err(|_| SAVE.to_string())?;
        output.write_all(contents.as_bytes()).and_then(|_| output.sync_all()).map_err(|_| SAVE.to_string())?;
    }
    WorkerState::initialize_separate_journal(next.clone(), next.journal_path.clone().ok_or(CONFIG)?).await?;
    read_new_baseline(&next).await?;
    Ok(RecordedProfile { text, config: next })
}

fn endpoint_link(file: &Path, value: &Endpoint) -> Result<PathBuf, String> {
    let ip: IpAddr = value.ip.parse().map_err(|_| CONFIG.to_string())?;
    Ok(file.parent().ok_or(CONFIG)?.join("local-runtime").join("endpoint-links")
        .join(format!("{}-{}-{}.toml", ip.to_string().replace(':', "_"), value.port, value.unit_id)))
}

fn remember_endpoint(file: &Path, text: &str, config: &SimulatorConfig) -> Result<(), String> {
    let path = endpoint_link(file, &endpoint(config)?)?;
    std::fs::create_dir_all(path.parent().ok_or(SAVE)?).map_err(|_| SAVE.to_string())?;
    let mut document: toml::Value = simulator_config::configuration_values(text)?;
    document["journal_path"] = toml::Value::String(profile_journal(file, config)?.to_string_lossy().into_owned());
    let after = toml::to_string(&document).map_err(|_| SAVE.to_string())?;
    if path.exists() {
        let before = std::fs::read_to_string(&path).map_err(|_| SAVE.to_string())?;
        replace(&path, &before, &after)
    } else {
        let temp = path.with_extension(format!("{}-{}.tmp", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().ok_or(SAVE)?));
        let mut output = std::fs::OpenOptions::new().create_new(true).write(true).open(&temp).map_err(|_| SAVE.to_string())?;
        output.write_all(after.as_bytes()).and_then(|_| output.sync_all()).map_err(|_| SAVE.to_string())?;
        drop(output);
        std::fs::rename(temp, path).map_err(|_| SAVE.to_string())
    }
}

fn retain_current_profile(file: &Path, text: &str, config: &SimulatorConfig) -> Result<(), String> {
    let managed = file.parent().ok_or(CONFIG)?.join("local-runtime").join("managed-sources");
    if !managed.exists() { return Ok(()); }
    let journal = profile_journal(file, config)?;
    let directory = journal.parent().ok_or(CONFIG)?;
    if directory.parent() != Some(managed.canonicalize().map_err(|_| CONFIG.to_string())?.as_path()) {
        return Ok(()); // Legacy external journals remain described by previous-config backups.
    }
    let target = directory.join("target-config.toml");
    if !target.exists() {
        // Preserve provenance before switching away from a legacy profile; never rewrite an existing record.
        let mut saved = std::fs::OpenOptions::new().create_new(true).write(true).open(target).map_err(|_| SAVE.to_string())?;
        saved.write_all(text.as_bytes()).and_then(|_| saved.sync_all()).map_err(|_| SAVE.to_string())?;
    }
    Ok(())
}

fn profile_journal(file: &Path, config: &SimulatorConfig) -> Result<PathBuf, String> {
    config.journal_path.as_ref().ok_or(CONFIG)?;
    let p = config.journal_file(file);
    Ok(p.parent().ok_or(CONFIG)?.canonicalize().map_err(|_| CONFIG.to_string())?.join(p.file_name().ok_or(CONFIG)?))
}

fn previous_endpoint(file: &Path, requested: &Endpoint) -> Result<Option<RecordedProfile>, String> {
    let root = file.parent().ok_or(CONFIG)?.join("local-runtime").join("managed-sources");
    if !root.exists() {
        return if endpoint_link(file, requested)?.exists() { Err("HISTORY_INCOMPLETE".into()) } else { Ok(None) };
    }
    let unknown = "HISTORY_INCOMPLETE";
    let (text, config) = read(file)?;
    let mut records = vec![RecordedProfile { text, config }];
    let link = endpoint_link(file, requested)?;
    let remembered = if link.exists() {
        let (text, mut config) = read(&link).map_err(|_| unknown.to_string())?;
        if endpoint(&config)? != *requested { return Err(unknown.into()); }
        config.journal_path = Some(profile_journal(file, &config).map_err(|_| unknown.to_string())?);
        Some(RecordedProfile { text, config })
    } else { None };
    let mut directories = Vec::new();
    for entry in std::fs::read_dir(&root).map_err(|_| unknown.to_string())? {
        let dir = entry.map_err(|_| unknown.to_string())?.path();
        if !dir.is_dir() { return Err(unknown.into()); }
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
        let journal = dir.canonicalize().map_err(|_| unknown.to_string())?.join("capture.json");
        if !records.iter().any(|r| profile_journal(file, &r.config).ok().as_ref() == Some(&journal)) {
            return Err(unknown.into());
        }
    }
    if let Some(remembered) = remembered {
        if !records.iter().any(|r| endpoint(&r.config).ok().as_ref() == Some(requested)
            && r.config.source_key == remembered.config.source_key
            && r.config.confirmed_session_id == remembered.config.confirmed_session_id
            && r.config.word_order.as_str() == remembered.config.word_order.as_str()
            && r.config.modbus_address_offset == remembered.config.modbus_address_offset
            && profile_journal(file, &r.config).ok() == remembered.config.journal_path) {
            return Err(unknown.into());
        }
        // This pointer records a completed/closed application association, not hardware identity.
        // The caller must still verify journal/DB and the live snapshot before resuming.
        return Ok(Some(remembered));
    }
    let mut found: Option<RecordedProfile> = None;
    for mut record in records {
        if endpoint(&record.config)? != *requested { continue; }
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
        } else { found = Some(record); }
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
    let trace = format!("{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default());
    switch_log(&trace, "SWITCH_START", "OBSERVED", format!("target_endpoint={}:{} unit={} reset_confirmed={}", value.ip, value.port, value.unit_id, reset_confirmed == Some(true)));
    let mut stage = "SWITCH_START";
    let result: Result<(), String> = async {
    let _guard = state.settings_gate.lock().await;
    stage = "WORKER_STOP_CHECK";
    switch_log(&trace, stage, if state.status.read().await.enabled { "WORKER_RUNNING" } else { "PASS" }, format!("expected_enabled=false actual_enabled={}", state.status.read().await.enabled));
    if state.status.read().await.enabled {
        return Err(BUSY.into());
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
    let changed = endpoint(&next)? != endpoint(&config)?;
    switch_log(&trace, "SOURCE_RESOLVED", "OBSERVED", format!("changed={changed} target_endpoint={}:{} unit={}", value.ip, value.port, value.unit_id));
    config.validate().map_err(|_| "Chưa thể xác minh máy đo mới. Vui lòng liên hệ kỹ thuật.".to_string())?;
    if !config.enabled {
        return Err("Chưa bật cấu hình thu dữ liệu. Vui lòng liên hệ kỹ thuật.".into());
    }
    let mut old_source_lock = None;
    let mut replacement = None;
    if changed {
        stage = "SOURCE_RESOLVED";
        let history = previous_endpoint(&file, &value).map_err(|e| match e.as_str() {
            "HISTORY_AMBIGUOUS" => "Địa chỉ này có nhiều lịch sử nguồn đo. Chưa thể chọn phiên an toàn; vui lòng liên hệ kỹ thuật.".to_string(),
            "HISTORY_INCOMPLETE" => "Thiếu cấu hình đối chiếu của một nguồn đã lưu. Chưa thể xác minh lịch sử địa chỉ máy đo.".to_string(),
            _ => e,
        })?;
        switch_log(&trace, "SOURCE_RESOLVED", if history.is_some() { "HISTORICAL_PROFILE" } else { "NEW_PROFILE" }, format!("endpoint={}:{} unit={} historical={}", value.ip, value.port, value.unit_id, history.is_some()));
        let pool = database.pool().await?;
        stage = "JOURNAL_CHECK";
        old_source_lock = Some(state.verify_source_closed(&file, &config, &pool, false, &trace).await?);
        if let Some(profile) = history {
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
                document["journal_path"] = toml::Value::String(next.journal_path.as_ref().ok_or(CONFIG)?.to_string_lossy().into_owned());
                replacement = Some(toml::to_string(&document).map_err(|_| SAVE.to_string())?);
            }
            drop(historical_lock);
        } else {
        read_new_baseline(&next).await?;
        let id = format!("qc-{}-{}", chrono::Utc::now().timestamp_nanos_opt().ok_or(SAVE)?, std::process::id());
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM plc_sources WHERE source_key=$1)")
            .bind(&id).fetch_one(&pool).await.map_err(|_| "Không thể kiểm tra nguồn dữ liệu mới.".to_string())?;
        if exists { return Err(SAVE.into()); }
        let directory = file.parent().ok_or(SAVE)?.join("local-runtime").join("managed-sources").join(&id);
        std::fs::create_dir_all(directory.parent().ok_or(SAVE)?).map_err(|_| SAVE.to_string())?;
        std::fs::create_dir(&directory).map_err(|_| SAVE.to_string())?;
        let journal = directory.join("capture.json");
        next.source_key = id.clone();
        next.confirmed_session_id = Some(format!("{id}-initial"));
        next.journal_path = Some(journal.clone());
        next.session_transition = None;
        next.validate().map_err(|_| CONFIG.to_string())?;
        let mut document: toml::Value = simulator_config::configuration_values(&text)?;
        document["address"] = toml::Value::String(next.address.clone());
        document["unit_id"] = toml::Value::Integer(next.unit_id.into());
        document["source_key"] = toml::Value::String(next.source_key.clone());
        document["confirmed_session_id"] = toml::Value::String(next.confirmed_session_id.clone().ok_or(CONFIG)?);
        document["journal_path"] = toml::Value::String(journal.to_string_lossy().into_owned());
        let after = toml::to_string(&document).map_err(|_| SAVE.to_string())?;
        simulator_config::parse(&after).and_then(|c| c.validate()).map_err(|_| CONFIG.to_string())?;
        // Preserve old configuration and journal; staged new files remain for diagnosis on failure.
        let mut backup = std::fs::OpenOptions::new().create_new(true).write(true).open(directory.join("previous-config.toml")).map_err(|_| SAVE.to_string())?;
        backup.write_all(text.as_bytes()).and_then(|_| backup.sync_all()).map_err(|_| SAVE.to_string())?;
        let mut target = std::fs::OpenOptions::new().create_new(true).write(true).open(directory.join("target-config.toml")).map_err(|_| SAVE.to_string())?;
        target.write_all(after.as_bytes()).and_then(|_| target.sync_all()).map_err(|_| SAVE.to_string())?;
        WorkerState::initialize_separate_journal(next.clone(), journal).await?;
        read_new_baseline(&next).await?;
        replacement = Some(after);
        }
    } else {
        stage = "PLC_SNAPSHOT";
        if state.verify_resume(&file, &config, false, &database, &trace).await? {
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
        retain_current_profile(&file, &text, &config)?;
        // The outgoing profile has passed closed-source checks. Keep its association for a later return.
        remember_endpoint(&file, &text, &config)?;
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
    drop(status);
    if failed {
        state.stop_collection().await?;
        if let Some(after) = &replacement {
            replace(&file, after, &text).map_err(|_| "Không thể khôi phục cấu hình cũ; hệ thống đã giữ trạng thái dừng. Vui lòng liên hệ kỹ thuật.".to_string())?;
            let _ = std::fs::remove_file(&pending);
        }
        return Err("Chưa thể khởi động thu dữ liệu. Vui lòng liên hệ kỹ thuật.".into());
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
    drop(old_source_lock);
    Ok(())
    }.await;
    match &result {
        Ok(()) => switch_log(&trace, "SWITCH_ALLOWED", "PASS", "configuration_committed=true worker_started=true"),
        Err(error) => switch_log(&trace, "SWITCH_BLOCKED", "GUARD_OR_OPERATION_FAILED", format!("failed_step={stage} error={error}")),
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
) -> Result<(), String> {
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
    probe(value.apply(&config)?).await
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
        "# Endpoint only. Technical commissioning is required before enabling collection.\nmode = \"modbus\"\nenabled = false\ntechnical_setup_confirmed = false\naddress = \"{}\"\nunit_id = {}\nack_enabled = false\npc_heartbeat_enabled = false\nallow_plc_connection = false\nallow_plc_heartbeat_write = false\n",
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
