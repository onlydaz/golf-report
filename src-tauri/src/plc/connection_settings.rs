//! One TOML-selected environment and one Worker. No live endpoint switching.
use super::{
    as200::{As200Error, map},
    simulator_config::{self, SimulatorConfig},
    worker::WorkerState,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

#[cfg(test)]
const PLC_ADDRESS: &str = "192.168.1.5:502";
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
    pub mapping_warning: Option<String>,
    pub message: String,
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

#[tauri::command]
pub async fn get_plc_connection_settings(
    state: tauri::State<'_, WorkerState>,
) -> Result<Settings, String> {
    let dir = directory(&state).await?;
    let (_, _, config) = current_config(&state, &dir).await?;
    Ok(settings(&config))
}

#[tauri::command]
pub async fn test_plc_connection(
    state: tauri::State<'_, WorkerState>,
) -> Result<InspectionReport, String> {
    let _gate = state.settings_gate.lock().await;
    let dir = ready(&state).await?;
    let (_, _, config) = current_config(&state, &dir).await?;
    let engine = state.engine_for_diagnostic_read().await;
    let _read_gate = match engine.as_ref() {
        Some(engine) => Some(engine.try_diagnostic_read_lock()?),
        None => None,
    };
    let report = match engine {
        Some(ref engine) => inspect_endpoint(engine.connection_config()).await,
        None => inspect_endpoint(config).await,
    };
    if report.as200_valid && engine.is_none() && state.tasks_are_stopped().await {
        state.status.write().await.confirm_read_only_probe();
    }
    Ok(report)
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
    let report = inspect_endpoint(config.clone()).await;
    if !report.as200_valid {
        state.record_connection("FAILED", &endpoint).await;
        return Err(report.mapping_warning.unwrap_or(report.message));
    }
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
    eprintln!(
        "PLC_INSPECT state=START endpoint={} unit_id={} control_start=D{} modbus_start={:?} offset={} word_order={}",
        config.address,
        config.unit_id,
        config.control_start,
        map::modbus_address(config.control_start, config.modbus_address_offset),
        config.modbus_address_offset,
        config.word_order.as_str()
    );
    let mut report = InspectionReport {
        tcp_connected: false,
        modbus_read_succeeded: false,
        as200_valid: false,
        mapping_warning: None,
        message: CONNECT.into(),
    };
    if let Err(error) = config.validate_read() {
        eprintln!(
            "PLC_INSPECT state=INVALID_CONFIG endpoint={} reason={error}",
            config.address
        );
        report.mapping_warning = Some(error);
        report.message =
            "Cấu hình vùng đọc chưa hợp lệ. Kiểm tra Register Map trước khi thử lại.".into();
        return report;
    }
    // Separate TCP reachability from a valid FC03 exchange. Both sockets are closed
    // after this one-shot inspection; no Worker or PLC writer is constructed.
    match super::as200::connect_tcp(
        &config.address,
        config.local_bind_ip,
        std::time::Duration::from_millis(config.timeout_ms),
    )
    .await
    {
        Ok(stream) => {
            report.tcp_connected = true;
            drop(stream);
        }
        Err(error) => {
            eprintln!(
                "PLC_INSPECT state=TCP_FAILED endpoint={} reason={error}",
                config.address
            );
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
            eprintln!(
                "PLC_INSPECT stage=CONTROL state={} endpoint={} control={:?} reason={}",
                if report.as200_valid {
                    "VALID"
                } else {
                    "INVALID"
                },
                config.address,
                snapshot.control,
                report
                    .mapping_warning
                    .as_deref()
                    .unwrap_or("stable control/profile; ACK requires Worker checkpoint validation")
            );
            report.message = if report.as200_valid {
                "TCP/Modbus và control AS200 hợp lệ. Lần kiểm tra chỉ đọc, không lưu PostgreSQL hoặc ghi ACK. Worker nhận raw kể cả payload chưa giải mã được."
            } else {
                "Đã đọc Modbus; control AS200 chưa hợp lệ. Kiểm tra Register Map và trạng thái PLC."
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
            report.mapping_warning = Some(format!(
                "Hết thời gian kiểm tra snapshot AS200 ({} ms).",
                config.timeout_ms.saturating_mul(10)
            ));
        }
    }
    eprintln!(
        "PLC_INSPECT state={} endpoint={} tcp_connected={} modbus_read_succeeded={} reason={}",
        if report.as200_valid {
            "COMPLETE"
        } else {
            "REJECTED"
        },
        config.address,
        report.tcp_connected,
        report.modbus_read_succeeded,
        report.mapping_warning.as_deref().unwrap_or(&report.message)
    );
    report
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
            // Ready=0 allows a provisional TotalCount during preparation. This
            // read-only probe cannot validate DB checkpoint/pending state; Worker
            // still requires that evidence before any Count/Done write.
            || (control.data_ready == 1 && control.pc_read_count > control.total_count)
            || !matches!(control.data_ready, 0 | 1)
            || !matches!(control.pc_read_done, 0 | 1)
        {
            return Err(format!(
                "Trạng thái AS200 không hợp lệ: TotalCount={}, PC_ReadCount={}, DataReady={}, PC_ReadDone={}. Cần counter không âm, cờ 0/1 và PC_ReadCount <= TotalCount khi DataReady=1.",
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

#[cfg(test)]
mod tests;
