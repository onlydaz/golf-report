use super::{
    as200::{
        As200Error, decode,
        reader::{As200Reader, Diagnostic, PollReport},
    },
    simulator_config::{self, SimulatorConfig},
};
use crate::{
    database::DatabaseState,
    models::{InboxInput, InboxRecord},
    repositories::{InboxRepository, SourceRepository},
    services::InboxService,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, Notify, RwLock, oneshot};
mod retry;

pub(super) fn switch_log(trace: &str, step: &str, reason: &str, detail: impl std::fmt::Display) {
    eprintln!("PLC_SWITCH trace={trace} step={step} reason={reason} {detail}");
}

// Compare immutable receipt data at PostgreSQL timestamp precision. SourceKey is resolved separately;
// source_id in a journal capture is a placeholder until the source is upserted by the sink.
fn processed_receipt_matches(row: &InboxRecord, input: &InboxInput) -> bool {
    row.session_id == input.confirmed_session_id && row.record_seq == input.record_seq
        && row.total_count == input.total_count && row.ingest_key == input.ingest_key
        && row.word_order == input.word_order.as_str() && row.buffer_index == input.buffer_index
        && row.received_at.timestamp_micros() == input.received_at.timestamp_micros()
        && serde_json::to_value(&input.raw_registers).is_ok_and(|raw| raw == row.raw_registers)
        && row.processing_status == "PROCESSED" && row.measurement_id.is_some() && row.processed_at.is_some()
}


fn verify_return_snapshot_with_receipt(journal: &Journal, report: &PollReport, receipt: Option<&InboxRecord>, trace: &str) -> Result<(), String> {
    let previous = journal.last_total.ok_or_else(|| {
        switch_log(trace, "BUFFER_VERIFY", "COUNTER_CHECKPOINT_MISSING", "expected=stored_last_total actual=none");
        "Chưa có mốc bộ đếm đã xác minh cho nguồn cũ."
    })?;
    if previous < 0 {
        switch_log(trace, "BUFFER_VERIFY", "COUNTER_CHECKPOINT_INVALID", format!("expected_nonnegative=true actual_last_total={previous}"));
        return Err("Lịch sử máy đo có bộ đếm không hợp lệ.".into());
    }
    let control = &report.control;
    if control.total_count < previous {
        switch_log(trace, "BUFFER_VERIFY", "COUNTER_MISMATCH", format!("expected_total_at_least={previous} actual_total={}", control.total_count));
        return Err(format!("Bộ đếm máy đo đã giảm từ {previous} xuống {}. Không có bằng chứng ranh giới phiên mới; không tự tạo phiên thay thế.", control.total_count));
    }
    let delta = control.total_count - previous;
    if delta > 0 && control.data_ready != 1 {
        switch_log(trace, "BUFFER_VERIFY", "DATA_READY_MISMATCH", format!("new_count={delta} expected_ready=1 actual_ready={}", control.data_ready));
        return Err("Dữ liệu máy đo mới không khớp trạng thái sẵn sàng. Chưa thể tiếp tục phiên cũ.".into());
    }
    if delta > 2 {
        switch_log(trace, "BUFFER_VERIFY", "BUFFER_CAPACITY_EXCEEDED", format!("new_count={delta} available_buffers=2"));
        return Err("Máy đo phát sinh nhiều hơn hai kết quả khi ngừng thu. Không thể chứng minh đủ dữ liệu từ hai buffer.".into());
    }
    for offset in 1..=delta {
        let sequence = previous + offset;
        if !report.buffers.iter().any(|b| b.record.as_ref().is_some_and(|r| r.record_seq == sequence && r.complete())) {
            switch_log(trace, "BUFFER_VERIFY", "BUFFER_RAW_NOT_AVAILABLE", format!("required_seq={sequence} present_sequences={:?}", report.buffers.iter().map(|b| b.record.as_ref().map(|r| r.record_seq)).collect::<Vec<_>>()));
            return Err("Máy đo thiếu bản ghi liên tiếp sau mốc đã lưu. Chưa thể tiếp tục thu.".into());
        }
    }
    if control.pc_read_done != 0 || control.pc_read_count != journal.ack.accepted {
        switch_log(trace, "ACK_CHECK", "ACK_CHECKPOINT_MISMATCH", format!("expected_read_count={} actual_read_count={} expected_done=0 actual_done={}", journal.ack.accepted, control.pc_read_count, control.pc_read_done));
        return Err("Trạng thái xác nhận trên máy đo không khớp lịch sử nguồn cũ.".into());
    }
    if previous == 0 && control.total_count == 0 {
        if control.data_ready != 0 || report.buffers.iter().any(|b| b.raw_registers.iter().any(|w| *w != 0)) {
            switch_log(trace, "BUFFER_VERIFY", "BASELINE_NOT_EMPTY", format!("expected_ready=0 actual_ready={} nonzero_buffers={:?}", control.data_ready, report.buffers.iter().map(|b| b.raw_registers.iter().any(|w| *w != 0)).collect::<Vec<_>>()));
            return Err("Máy đo không còn ở baseline rỗng đã xác minh.".into());
        }
    } else if previous > 0 && !report.buffers.iter().any(|b| b.record.as_ref().is_some_and(|r| r.record_seq == previous)
        && (journal.deliveries.iter().any(|d| d.input.confirmed_session_id == journal.session_id
            && d.input.record_seq == previous && d.input.raw_registers == b.raw_registers)
            || receipt.is_some_and(|row| row.record_seq == previous
                && row.session_id == journal.session_id
                && row.processing_status == "PROCESSED"
                && row.measurement_id.is_some() && row.processed_at.is_some()
                && row.buffer_index == b.buffer_index as i32
                && serde_json::to_value(&b.raw_registers).is_ok_and(|raw| raw == row.raw_registers)))) {
        let present = report.buffers.iter().any(|b| b.record.as_ref().is_some_and(|r| r.record_seq == previous));
        let journal_raw = journal.deliveries.iter().any(|d| d.input.confirmed_session_id == journal.session_id && d.input.record_seq == previous);
        let historical_raw = journal_raw || receipt.is_some();
        switch_log(trace, "BUFFER_VERIFY", if present && historical_raw { "BUFFER_RAW_MISMATCH" } else { "BUFFER_RAW_NOT_AVAILABLE" }, format!("required_seq={previous} record_present={present} journal_raw={journal_raw} processed_receipt={} expected=exact_raw_and_slot_match", receipt.is_some()));
        return Err("Không còn bản ghi đối chiếu nguyên vẹn của phiên cũ trên máy đo. Chưa thể tiếp tục.".into());
    }
    switch_log(trace, "BUFFER_VERIFY", "PASS", format!("required_seq={previous} journal_raw={} processed_receipt={}", journal.deliveries.iter().any(|d| d.input.record_seq == previous), receipt.is_some()));
    Ok(())
}

#[cfg(test)]
fn verify_return_snapshot(journal: &Journal, report: &PollReport) -> Result<(), String> {
    verify_return_snapshot_with_receipt(journal, report, None, "unit-test")
}

fn latest_replaces(current: Option<&LatestRecord>, incoming: &LatestRecord) -> bool {
    current.is_none_or(|current| {
        if incoming.session_id.is_some()
            && incoming.source_key == current.source_key
            && incoming.session_id == current.session_id
        {
            incoming.record_seq >= current.record_seq
        } else {
            // A different session is never ordered by its counter. Caller validates active session.
            incoming.session_id != current.session_id || incoming.received_at >= current.received_at
        }
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestRecord {
    pub source_key: String,
    pub session_id: Option<String>,
    pub delivery_id: String,
    pub inbox_id: Option<i64>,
    pub measurement_id: Option<i64>,
    pub record_seq: i32,
    pub total_count: Option<i64>,
    pub product_code: String,
    pub club_number: i32,
    pub received_at: DateTime<Utc>,
    pub overall_result: String,
    pub tip_result: String,
    pub butt_result: String,
    pub save_state: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerStatus {
    pub manual_retry_results: Vec<ManualRetryRequest>,
    pub retry_diagnostics: Vec<String>,
    pub blocked_inbox_ids: Vec<i64>,
    pub blocked_inbox_count: usize,
    pub retry_waiting_count: usize,
    pub plc_health: String,
    pub pc_heartbeat_status: String,
    pub ack_status: &'static str,
    pub enabled: bool,
    pub address: String,
    pub unit_id: u8,
    pub source_key: String,
    pub session_id: Option<String>,
    pub connection: String,
    pub state: String,
    pub message: String,
    pub pause_reason: Option<String>,
    pub warnings: Vec<String>,
    pub volatile_count: usize,
    pub latest: Option<LatestRecord>,
    pub last_saved_id: Option<i64>,
}
impl Default for WorkerStatus {
    fn default() -> Self {
        Self {
            manual_retry_results: Vec::new(),
            blocked_inbox_ids: Vec::new(),
            retry_diagnostics: Vec::new(),
            blocked_inbox_count: 0,
            retry_waiting_count: 0,
            plc_health: "UNCONFIGURED".into(),
            pc_heartbeat_status: "DISABLED".into(),
            ack_status: "READ_ONLY_PROTOCOL_UNCONFIRMED",
            enabled: false,
            address: String::new(),
            unit_id: 1,
            source_key: String::new(),
            session_id: None,
            connection: "DISABLED".into(),
            state: "DISABLED".into(),
            message: "Chưa thiết lập kết nối. Vào Cài đặt kết nối để nhập thông tin thiết bị đo."
                .into(),
            pause_reason: None,
            warnings: Vec::new(),
            volatile_count: 0,
            latest: None,
            last_saved_id: None,
        }
    }
}
#[derive(Clone, Default)]
pub struct WorkerState {
    stopping: Arc<AtomicBool>,
    tasks: Arc<Mutex<Vec<tauri::async_runtime::JoinHandle<()>>>>,
    database: Arc<Mutex<Option<DatabaseState>>>,
    pub(super) settings_gate: Arc<Mutex<()>>,
    pub(super) settings_path: Arc<Mutex<Option<PathBuf>>>,
    pub(super) settings_directory: Arc<Mutex<Option<PathBuf>>>,
    pub(super) settings_ready: Arc<AtomicBool>,
    pub(super) runtime_error: Arc<Mutex<Option<String>>>,
    pub(super) settings_restart: Arc<AtomicBool>,
    heartbeat: Arc<std::sync::Mutex<Option<super::as200::heartbeat::HeartbeatMonitor>>>,
    engine: Arc<Mutex<Option<std::sync::Weak<Engine>>>>,
    pub status: Arc<RwLock<WorkerStatus>>,
    ack_gap: Arc<AtomicBool>,
    reconnect_now: Arc<Notify>,
    reconnect_waiters: Arc<Mutex<Vec<oneshot::Sender<Result<(), String>>>>>,
}
impl WorkerState {
    pub(crate) async fn report_runtime_initialization_failure(&self, error: String) {
        *self.runtime_error.lock().await = Some(error.clone());
        // A failed runtime check must not enable first-run setup commands.
        self.settings_ready.store(false, Ordering::SeqCst);
        let mut status = self.status.write().await;
        status.enabled = false;
        status.state = "ERROR".into();
        status.message = error;
    }

    pub(super) async fn stop_collection(&self) -> Result<(), String> {
        let path = self.settings_path.lock().await.clone().ok_or("Chưa có cấu hình kết nối.")?;
        let engine = self
            .engine
            .lock()
            .await
            .as_ref()
            .and_then(|engine| engine.upgrade());
        let marker = OpenOptions::new().create(true).truncate(false).write(true)
            .open(path.with_extension("operator-stop")).map_err(|_| "Không thể ghi nhận yêu cầu dừng.")?;
        marker.sync_all().map_err(|_| "Không thể lưu yêu cầu dừng.")?;
        self.stopping.store(true, Ordering::SeqCst);
        self.status.write().await.state = "STOPPING".into();
        // Keep handles in shared state: cancelling the command must not detach unfinished tasks.
        let mut tasks = self.tasks.lock().await;
        let mut task_error = None;
        while !tasks.is_empty() {
            if (&mut tasks[0]).await.is_err() {
                task_error = Some("Worker gặp lỗi khi dừng.".to_string());
            }
            tasks.remove(0);
        }
        tasks.clear();
        for waiter in std::mem::take(&mut *self.reconnect_waiters.lock().await) {
            let _ = waiter.send(Err("Đã dừng thu dữ liệu; không tự kết nối lại.".into()));
        }
        let flush_result = if let (Some(engine), Some(database)) =
            (engine, self.database.lock().await.clone())
        {
            match tokio::time::timeout(Duration::from_secs(8), async {
                let pool = database.pool().await?;
                engine.flush(&pool).await
            })
            .await
            {
                Ok(result) => result,
                Err(_) => Err("Hết thời gian lưu dữ liệu đang chờ vào PostgreSQL.".into()),
            }
        } else {
            Ok(())
        };
        let stop_result = task_error.map_or(flush_result, Err);
        *self.engine.lock().await = None;
        let mut status = self.status.write().await;
        status.enabled = false;
        status.connection = "DISABLED".into();
        status.state = "STOPPED".into();
        status.message = if stop_result.is_ok() {
            "Đã dừng thu dữ liệu.".into()
        } else {
            "Đã dừng thu; dữ liệu chưa lưu vẫn được giữ trong journal để phục hồi.".into()
        };
        stop_result
    }
    async fn fresh_health(&self) -> bool {
        let mut status = self.status.write().await;
        let health = self
            .heartbeat
            .lock()
            .unwrap()
            .as_ref()
            .map(|m| m.health(std::time::Instant::now()));
        if status.plc_health != "DISCONNECTED" {
            if let Some(health) = health {
                status.plc_health = health.into();
            }
        }
        !status.enabled || (health == Some("RUNNING") && status.plc_health == "RUNNING")
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Delivery {
    input: InboxInput,
    inbox_id: Option<i64>,
    terminal: bool,
}
/// Chẩn đoán bền vững, không phải delivery và không bao giờ là bằng chứng ACK.
#[derive(Clone, Serialize, Deserialize)]
struct QuarantinedRecord {
    #[serde(default)]
    control: Option<super::as200::model::PlcControl>,
    source_key: String,
    session_id: Option<String>,
    address: String,
    unit_id: u8,
    modbus_address_offset: i32,
    word_order: crate::models::StoredWordOrder,
    buffer_index: usize,
    record_seq: Option<i32>,
    total_count: i32,
    received_at: DateTime<Utc>,
    raw_registers: Vec<u16>,
    error: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct SessionArchive {
    session_id: Option<String>,
    next_session_id: Option<String>,
    last_total: Option<i32>,
    ack: super::as200::ack::AckProgress,
    deliveries: Vec<Delivery>,
    warnings: Vec<String>,
    reason: String,
    confirmed_at: DateTime<Utc>,
}
#[derive(Clone, Serialize, Deserialize)]
struct SessionRequest {
    requested_session_id: Option<String>,
    previous_pause: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualRetryRequest {
    pub inbox_id: i64,
    pub state: String,
    pub error: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Journal {
    #[serde(default)]
    manual_retries: Vec<ManualRetryRequest>,
    #[serde(default)]
    session_request: Option<SessionRequest>,
    #[serde(default)]
    retries: Vec<retry::RetryEntry>,
    #[serde(default)]
    session_archives: Vec<SessionArchive>,
    #[serde(default)]
    quarantine: Vec<QuarantinedRecord>,
    #[serde(default)]
    previous_session_warnings: Vec<String>,
    #[serde(default)]
    ack: super::as200::ack::AckProgress,
    version: u8,
    source_key: String,
    session_id: Option<String>,
    last_total: Option<i32>,
    pause: Option<String>,
    warnings: Vec<String>,
    deliveries: Vec<Delivery>,
}
impl Journal {
    // A reset may be isolated to its source only after the old ACK checkpoint is
    // complete. Keep the pause and any empty-buffer evidence in that journal.
    fn has_reconciliable_reset_diagnostics(&self, config: &SimulatorConfig) -> bool {
        self.pause.as_deref() == Some("RESET")
            && self.last_total.is_some_and(|total| total > 0 && self.ack.accepted == total)
            && self.ack.intent.is_none()
            && self.quarantine.iter().all(|q| {
                q.source_key == config.source_key
                    && q.session_id == config.confirmed_session_id
                    && q.address == config.address && q.unit_id == config.unit_id
                    && q.modbus_address_offset == config.modbus_address_offset
                    && q.word_order.as_str() == config.word_order.as_str()
                    && q.buffer_index < super::as200::map::BUFFER_COUNT
                    && q.total_count == 0
                    && q.error == "TotalCount giảm; cần xác nhận phiên/reset PLC trước khi tiếp tục"
                    && q.raw_registers.len() == super::as200::map::RECORD_SIZE
                    && q.raw_registers.iter().all(|word| *word == 0)
                    && q.control.as_ref().is_some_and(|c| {
                        c.format_version == 1 && c.record_size == 240 && c.buffer_count == 2
                            && c.total_count == 0 && c.pc_read_count == 0
                            && c.pc_read_done == 0 && c.data_ready == 0
                    })
            })
            && self.warnings.iter().all(|warning| warning.starts_with("RESET_CONTEXT:")
                || warning.starts_with("RESET:")
                || self.quarantine.iter().any(|q|
                    *warning == format!("QUARANTINE: đã giữ dữ liệu lỗi tại buffer {} để đối chiếu; chưa gửi xác nhận đã đọc.", q.buffer_index)))
    }
}
pub(super) struct Engine {
    config: SimulatorConfig,
    path: PathBuf,
    journal: Mutex<Journal>,
    state: WorkerState,
    _lock: File,
    storage_verified: AtomicBool,
}
impl Engine {
    fn open(
        config: SimulatorConfig,
        path: PathBuf,
        state: WorkerState,
    ) -> Result<Arc<Self>, String> {
        config.validate()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))
            .map_err(|e| e.to_string())?;
        lock.try_lock().map_err(|e| {
            format!("Journal đã được worker khác giữ khóa hoặc không khóa được: {e}")
        })?;
        let mut journal = if path.exists() {
            serde_json::from_str::<Journal>(
                &std::fs::read_to_string(&path).map_err(|e| e.to_string())?,
            )
            .map_err(|e| format!("Journal lỗi; không ghi đè: {e}"))?
        } else {
            Journal {
                manual_retries: Vec::new(),
                session_request: None,
                retries: Vec::new(),
                session_archives: Vec::new(),
                quarantine: Vec::new(),
                previous_session_warnings: Vec::new(),
                ack: Default::default(),
                version: 2,
                source_key: config.source_key.clone(),
                session_id: config.confirmed_session_id.clone(),
                last_total: None,
                pause: None,
                warnings: Vec::new(),
                deliveries: Vec::new(),
            }
        };
        if !matches!(journal.version, 1 | 2) || journal.source_key != config.source_key {
            return Err("Journal sai version/SourceKey. Dùng journal riêng cho từng thiết bị; không tự xóa journal cũ.".into());
        }
        // Đọc được v1 nhưng lần persist tiếp theo dùng v2. Binary cũ sẽ từ chối
        // thay vì bỏ qua trường quarantine rồi ghi mất chẩn đoán khi downgrade.
        journal.version = 2;
        if journal.session_id == config.confirmed_session_id
            && journal.pause.as_deref() == Some("SESSION_RECONCILIATION")
        {
            if let Some(request) = journal.session_request.take() {
                journal.pause = request.previous_pause;
                let message = format!(
                    "SESSION_REQUEST_CANCELLED: source={}, active={:?}, requested={:?}; giữ nguyên ACK intent/checkpoint và khôi phục pause trước yêu cầu",
                    journal.source_key, journal.session_id, request.requested_session_id
                );
                Self::warn(&mut journal, message);
            }
        }
        if journal.session_id != config.confirmed_session_id {
            if journal
                .pause
                .as_deref()
                .is_some_and(|p| !matches!(p, "RESET" | "SESSION_RECONCILIATION"))
            {
                return Err(format!(
                    "Không dùng phiên mới để vượt lỗi: source={}, old={:?}, requested={:?}, pause={:?}, total={:?}, ACK={}",
                    journal.source_key,
                    journal.session_id,
                    config.confirmed_session_id,
                    journal.pause,
                    journal.last_total,
                    journal.ack.accepted
                ));
            }
            if journal.session_request.is_none() {
                journal.session_request = Some(SessionRequest {
                    requested_session_id: config.confirmed_session_id.clone(),
                    previous_pause: journal.pause.clone(),
                });
            } else if let Some(request) = journal.session_request.as_mut() {
                request.requested_session_id = config.confirmed_session_id.clone();
            }
            journal.pause = Some("SESSION_RECONCILIATION".into());
            let message = format!(
                "SESSION_RECONCILIATION: source={}, journal_session={:?}, requested={:?}, journal_total={:?}, ack={}, intent={:?}. Cần đối chiếu cấu hình session_transition, PostgreSQL và snapshot PLC; chưa thay phiên/checkpoint.",
                journal.source_key,
                journal.session_id,
                config.confirmed_session_id,
                journal.last_total,
                journal.ack.accepted,
                journal.ack.intent.as_ref().map(|i| i.target)
            );
            Self::warn(&mut journal, message);
        }
        let engine = Arc::new(Self {
            config,
            path,
            journal: Mutex::new(journal),
            state,
            _lock: lock,
            storage_verified: AtomicBool::new(false),
        });
        Ok(engine)
    }
    fn persist(&self, journal: &Journal) -> Result<(), String> {
        let tmp = self.path.with_extension("tmp");
        let mut file = File::create(&tmp).map_err(|e| e.to_string())?;
        file.write_all(&serde_json::to_vec(journal).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        std::fs::rename(tmp,&self.path).map_err(|e|format!("Không lưu được journal: {e}. Dữ liệu chưa vào inbox có nguy cơ mất khi ứng dụng đóng."))
    }
    fn warn(journal: &mut Journal, message: String) {
        if !journal.warnings.contains(&message) {
            journal.warnings.push(message);
        }
    }
    async fn publish_journal(&self) -> Result<(), String> {
        let journal = self.journal.lock().await;
        let mut status = self.state.status.write().await;
        status.volatile_count = journal
            .deliveries
            .iter()
            .filter(|d| d.inbox_id.is_none())
            .count();
        status.warnings = journal.warnings.clone();
        status.manual_retry_results = journal.manual_retries.clone();
        status.blocked_inbox_count = journal
            .retries
            .iter()
            .filter(|r| r.blocked || r.attempts >= self.config.retry_max_attempts)
            .count();
        status.blocked_inbox_ids = journal
            .retries
            .iter()
            .filter(|r| r.blocked || r.attempts >= self.config.retry_max_attempts)
            .map(|r| r.inbox_id)
            .collect();
        status.retry_waiting_count = journal.retries.len() - status.blocked_inbox_count;
        status.retry_diagnostics = journal
            .retries
            .iter()
            .map(|r| {
                format!(
                    "inbox={}, source={}, session={:?}, attempts={}, blocked={}, next={}, error={}",
                    r.inbox_id,
                    r.source_id,
                    r.session_id,
                    r.attempts,
                    r.blocked || r.attempts >= self.config.retry_max_attempts,
                    r.next_attempt_at,
                    r.error
                )
            })
            .collect();
        status.session_id = journal.session_id.clone();
        status.pause_reason = journal.pause.clone();
        if journal.pause.is_some() {
            status.state = "PAUSED".into();
            status.message="Worker đã tạm dừng tiếp nhận; cần kiểm tra cảnh báo. Dữ liệu đã chụp vẫn được retry vào inbox.".into();
        }
        self.persist(&journal)
    }
    async fn error(&self, message: String) {
        let mut status = self.state.status.write().await;
        status.state = if status.pause_reason.is_some() {
            "PAUSED"
        } else {
            "ERROR"
        }
        .into();
        status.message = message;
    }
    async fn read_error(&self, error: As200Error) {
        if error == As200Error::SessionUnconfirmed {
            let mut journal = self.journal.lock().await;
            journal.pause = Some("RESET".into());
            let context = format!(
                "RESET_CONTEXT: source={}, session={:?}, configured={:?}, journal_total={:?}, ack={}, intent={:?}, receipts={}, pause={:?}",
                journal.source_key,
                journal.session_id,
                self.config.confirmed_session_id,
                journal.last_total,
                journal.ack.accepted,
                journal.ack.intent.as_ref().map(|i| i.target),
                journal.deliveries.len(),
                journal.pause
            );
            eprintln!("{context}");
            Self::warn(&mut journal, context);
            Self::warn(&mut journal,"RESET: counter giảm/phiên chưa xác nhận. Không tạo session mới tự động; cần kỹ thuật đối chiếu phiên và trạng thái thiết bị.".into());
            drop(journal);
            let _ = self.publish_journal().await;
        } else {
            self.state.status.write().await.connection = match error {
                As200Error::Transport(_) | As200Error::Timeout => "DISCONNECTED",
                _ => "CONNECTED",
            }
            .into();
            self.error(error.to_string()).await;
        }
    }
    fn quarantine_snapshot(&self, journal: &mut Journal, report: &PollReport) -> Vec<usize> {
        let tracker_error = report.diagnostics.iter().find_map(|d| match d {
            Diagnostic::TrackerRejected { error, .. } => Some(error.clone()),
            _ => None,
        });
        let mut invalid = Vec::new();
        for buffer in &report.buffers {
            if tracker_error.is_some()
                || buffer
                    .raw_registers
                    .get(super::as200::map::record::STATUS)
                    .is_some_and(|v| v & 1 != 0)
            {
                let error = tracker_error.clone().or_else(|| match &buffer.record {
                    Some(record) => record
                        .validate_metadata(Some(i64::from(report.control.total_count)))
                        .err()
                        .or_else(|| record.validate_workflow().err())
                        .or_else(|| {
                            (!record.complete()).then(|| {
                                "Bản ghi chưa hoàn tất; chưa xác nhận quy trình kết thúc sớm"
                                    .to_string()
                            })
                        }),
                    None => Some(
                        report
                            .diagnostics
                            .iter()
                            .find_map(|d| match d {
                                Diagnostic::InvalidRecord {
                                    buffer_index,
                                    error,
                                } if *buffer_index == buffer.buffer_index => Some(error.clone()),
                                _ => None,
                            })
                            .unwrap_or_else(|| "Không giải mã được bản ghi".into()),
                    ),
                });
                if let Some(error) = error {
                    invalid.push(buffer.buffer_index);
                    if !journal.quarantine.iter().any(|q| {
                        q.session_id == self.config.confirmed_session_id
                            && q.buffer_index == buffer.buffer_index
                            && q.raw_registers == buffer.raw_registers
                            && q.error == error
                            && q.control.as_ref().is_some_and(|c| {
                                c.same_buffer_state(&report.control)
                                    && c.pc_read_count == report.control.pc_read_count
                                    && c.pc_read_done == report.control.pc_read_done
                                    && c.reserve == report.control.reserve
                            })
                    }) {
                        let sequence = decode::dint32(
                            &buffer.raw_registers,
                            super::as200::map::record::SEQUENCE,
                            self.config.reader().word_order,
                        )
                        .ok();
                        journal.quarantine.push(QuarantinedRecord {
                            control: Some(report.control.clone()),
                            source_key: self.config.source_key.clone(),
                            session_id: self.config.confirmed_session_id.clone(),
                            address: self.config.address.clone(),
                            unit_id: self.config.unit_id,
                            modbus_address_offset: self.config.modbus_address_offset,
                            word_order: self.config.word_order,
                            buffer_index: buffer.buffer_index,
                            record_seq: sequence,
                            total_count: report.control.total_count,
                            received_at: Utc::now(),
                            raw_registers: buffer.raw_registers.clone(),
                            error,
                        });
                        let quarantine = journal.quarantine.last().expect("quarantine vừa được thêm");
                        let nonzero_words = quarantine
                            .raw_registers
                            .iter()
                            .filter(|word| **word != 0)
                            .count();
                        switch_log(
                            "worker",
                            "QUARANTINE",
                            "RECORDED",
                            format!(
                                "buffer={} record_seq={:?} total={} error={} received_at={} raw_words={} nonzero_words={} raw_checksum={} control={:?}",
                                quarantine.buffer_index,
                                quarantine.record_seq,
                                quarantine.total_count,
                                quarantine.error,
                                quarantine.received_at,
                                quarantine.raw_registers.len(),
                                nonzero_words,
                                quarantine.raw_registers.iter().fold(0u32, |sum, word| {
                                    sum.wrapping_add(u32::from(*word))
                                }),
                                quarantine.control.as_ref().map(|control| (
                                    control.total_count,
                                    control.pc_read_count,
                                    control.pc_read_done,
                                    control.data_ready
                                ))
                            ),
                        );
                        Self::warn(
                            journal,
                            format!(
                                "QUARANTINE: đã giữ dữ liệu lỗi tại buffer {} để đối chiếu; chưa gửi xác nhận đã đọc.",
                                buffer.buffer_index
                            ),
                        );
                    }
                    // Không xóa chẩn đoán cũ để vượt giới hạn; dừng đọc sau snapshot này.
                    if journal.quarantine.len() >= self.config.max_queue {
                        journal.pause = Some("DIAGNOSTIC_FULL".into());
                        Self::warn(journal, "DIAGNOSTIC_FULL: vùng chẩn đoán đã đầy; giữ dữ liệu và liên hệ kỹ thuật, không tự xóa hoặc đổi phiên để vượt lỗi.".into());
                    }
                    continue;
                }
            }
        }
        invalid
    }
    async fn capture(&self, mut report: PollReport) -> Result<(), String> {
        {
            let mut status = self.state.status.write().await;
            if status.connection == "DISCONNECTED" {
                status.state = "WAITING".into();
                status.message = "AS200 đã kết nối lại; tiếp tục theo dõi dữ liệu.".into();
            }
            status.connection = "CONNECTED".into();
        }
        let mut journal = self.journal.lock().await;
        let was_paused = journal.pause.is_some();
        let mut identities = std::collections::HashSet::new();
        let conflict = report
            .buffers
            .iter()
            .filter_map(|b| b.record.as_ref())
            .any(|r| {
                !identities.insert(r.record_seq)
                    || journal.deliveries.iter().any(|d| {
                        d.input.confirmed_session_id == journal.session_id
                            && d.input.record_seq == r.record_seq
                            && d.input.raw_registers != r.raw_registers
                    })
            });
        if conflict {
            report.diagnostics.push(Diagnostic::TrackerRejected {
                error: "CONFLICT: snapshot/journal identity mismatch".into(),
                session_unconfirmed: false,
            });
        }
        let invalid = self.quarantine_snapshot(&mut journal, &report);
        if let Some(reset) = report.diagnostics.iter().find_map(|d| match d {
            Diagnostic::TrackerRejected {
                session_unconfirmed,
                ..
            } => Some(*session_unconfirmed),
            _ => None,
        }) {
            journal.pause = Some(if reset { "RESET" } else { "CONFLICT" }.into());
            drop(journal);
            return self.publish_journal().await; // Preserve both slots before rejecting identity.
        }
        let has_invalid = !invalid.is_empty();
        if was_paused {
            drop(journal);
            return self.publish_journal().await;
        }
        if journal
            .last_total
            .is_some_and(|last| report.control.total_count < last)
            || report.control.total_count < 0
        {
            journal.pause = Some("RESET".into());
            let context = format!(
                "RESET_CONTEXT: source={}, session={:?}, journal_total={:?}, remote_total={}, remote_read_count={}, remote_done={}, ack={}, intent={:?}, receipts={}",
                journal.source_key,
                journal.session_id,
                journal.last_total,
                report.control.total_count,
                report.control.pc_read_count,
                report.control.pc_read_done,
                journal.ack.accepted,
                journal.ack.intent.as_ref().map(|i| i.target),
                journal.deliveries.len()
            );
            eprintln!("{context}");
            Self::warn(&mut journal, context);
            Self::warn(&mut journal,"RESET: TotalCount thấp hơn checkpoint journal. Giữ session cũ không an toàn; cần xác nhận phiên mới.".into());
            drop(journal);
            return self.publish_journal().await;
        }
        let gap = journal
            .last_total
            .is_some_and(|last| report.control.total_count - last > 2)
            || report.diagnostics.iter().any(|d| {
                matches!(
                    d,
                    Diagnostic::BufferOverrunRisk { .. } | Diagnostic::UnaccountedRecords { .. }
                )
            });
        if gap {
            let previous = journal.last_total;
            Self::warn(
                &mut journal,
                format!(
                    "BUFFER GAP: checkpoint={previous:?}, TotalCount={}, diagnostics={:?}. Chỉ giữ dữ liệu thực còn trong hai buffer; có nguy cơ mất bản ghi.",
                    report.control.total_count, report.diagnostics
                ),
            );
        }
        if journal.last_total.is_none()
            && journal.deliveries.is_empty()
            && report.control.total_count > 2
        {
            Self::warn(&mut journal,"BASELINE: chưa có checkpoint cục bộ; chỉ đọc được các record còn trong hai buffer. Không thể xác nhận đầy đủ lịch sử trước lúc bật worker.".into());
        }
        let mut sequences = Vec::new();
        for buffer in report.buffers {
            if invalid.contains(&buffer.buffer_index) {
                continue;
            }
            let Some(record) = buffer.record.filter(|r| r.complete()) else {
                continue;
            };
            sequences.push(record.record_seq);
            if let Some(existing) = journal.deliveries.iter().find(|d| {
                d.input.confirmed_session_id == self.config.confirmed_session_id
                    && d.input.record_seq == record.record_seq
            }) {
                if existing.input.raw_registers != record.raw_registers {
                    journal.pause = Some("CONFLICT".into());
                    Self::warn(
                        &mut journal,
                        format!(
                            "CONFLICT: RecordSeq {} có raw data thay đổi trong cùng phiên; không tạo delivery mới để vượt lỗi.",
                            record.record_seq
                        ),
                    );
                }
                continue;
            }
            if journal.deliveries.iter().filter(|d| !d.terminal).count() >= self.config.max_queue {
                journal.pause = Some("QUEUE_FULL".into());
                Self::warn(&mut journal,"Hàng chờ đầy; dừng tiếp nhận để tránh bỏ dữ liệu. Cần khôi phục PostgreSQL và kiểm tra buffer trước khi mở lại ứng dụng.".into());
                break;
            }
            let received_at = Utc::now();
            let key = format!(
                "delivery:{}:{}:{}",
                received_at
                    .timestamp_nanos_opt()
                    .unwrap_or(received_at.timestamp_micros()),
                record.buffer_index,
                record.record_seq
            );
            let input = InboxInput {
                source_id: 0,
                confirmed_session_id: self.config.confirmed_session_id.clone(),
                ingest_key: key.clone(),
                record_seq: record.record_seq,
                total_count: Some(i64::from(report.control.total_count)),
                buffer_index: record.buffer_index as i32,
                raw_registers: record.raw_registers,
                word_order: self.config.word_order,
                received_at,
            };
            let latest = LatestRecord {
                source_key: self.config.source_key.clone(),
                session_id: journal.session_id.clone(),
                delivery_id: key,
                inbox_id: None,
                measurement_id: None,
                record_seq: record.record_seq,
                total_count: input.total_count,
                product_code: record.product_code,
                club_number: record.stick_no,
                received_at,
                overall_result: format!("{:?}", record.overall_result).to_uppercase(),
                tip_result: format!("{:?}", record.tip.result).to_uppercase(),
                butt_result: format!("{:?}", record.butt.result).to_uppercase(),
                save_state: "QUEUED".into(),
            };
            journal.deliveries.push(Delivery {
                input,
                inbox_id: None,
                terminal: false,
            });
            {
                let mut status = self.state.status.write().await;
                if latest_replaces(status.latest.as_ref(), &latest) {
                    status.latest = Some(latest);
                }
                status.state = "QUEUED".into();
                status.message =
                    "Đã nhận bản ghi hoàn thành; đang chờ xác nhận lưu inbox PostgreSQL.".into();
            }
        }
        let incomplete = report
            .diagnostics
            .iter()
            .any(|d| matches!(d, Diagnostic::IncompleteRecord { .. }));
        if !incomplete && !has_invalid && journal.pause.as_deref() != Some("QUEUE_FULL") {
            journal.last_total = Some(report.control.total_count);
        }
        if gap && journal.pause.as_deref() != Some("DIAGNOSTIC_FULL") {
            journal.pause = Some("OVERFLOW".into());
        }
        let ack_checkpoint = journal.ack.accepted;
        journal.deliveries.retain(|d| {
            !d.terminal
                || sequences.contains(&d.input.record_seq)
                || (self.config.ack_enabled
                    && d.input.confirmed_session_id == self.config.confirmed_session_id
                    && d.input.record_seq > ack_checkpoint)
        });
        drop(journal);
        {
            let mut status = self.state.status.write().await;
            if status.latest.is_none() {
                status.state = "WAITING".into();
                status.message = "Đã kết nối thiết bị đo, đang chờ bản ghi hoàn thành.".into();
            }
        }
        self.publish_journal().await
    }
    fn session_transition_candidate(
        &self,
        journal: &Journal,
        report: &PollReport,
        unused_in_database: bool,
    ) -> Result<Journal, String> {
        if journal.pause.as_deref() != Some("SESSION_RECONCILIATION") {
            return Err(
                "SESSION_RECONCILIATION: trạng thái journal đã thay đổi, cần kiểm tra lại".into(),
            );
        }
        if journal.session_id == self.config.confirmed_session_id {
            return Err("SESSION_RECONCILIATION: không phải yêu cầu phiên mới".into());
        }
        let change = self
            .config
            .session_transition
            .as_ref()
            .ok_or("SESSION_RECONCILIATION: chưa có yêu cầu đối chiếu phiên cụ thể")?;
        if journal.ack.intent.is_some() {
            return Err("SESSION_RECONCILIATION: ACK intent phiên cũ chưa được giải quyết; không cho phép phiên mới giải phóng intent".into());
        }
        if !unused_in_database
            || journal.deliveries.iter().any(|d| d.inbox_id.is_none())
            || journal.session_id != change.previous_session_id
            || journal.last_total != change.previous_total_count
            || journal.ack.accepted != change.previous_ack_count
        {
            return Err("SESSION_RECONCILIATION: lịch sử/receipt/checkpoint không khớp hoặc tên phiên đã được sử dụng".into());
        }
        let c = &report.control;
        if c.total_count != change.expected_total_count
            || c.pc_read_count != 0
            || c.pc_read_done != 0
            || c.data_ready != i16::from(c.total_count > 0)
            || !report.diagnostics.is_empty()
        {
            return Err(format!(
                "SESSION_RECONCILIATION: remote Total={}, Count={}, Done={}, Ready={} không khớp yêu cầu; giữ nguyên journal",
                c.total_count, c.pc_read_count, c.pc_read_done, c.data_ready
            ));
        }
        // Phiên mới phải bắt đầu từ mốc rỗng hoặc còn đủ prefix 1..Total trong hai slot.
        for seq in 1..=c.total_count {
            let r = report
                .buffers
                .iter()
                .filter_map(|b| b.record.as_ref())
                .find(|r| r.record_seq == seq)
                .ok_or("SESSION_RECONCILIATION: thiếu prefix phiên mới")?;
            r.validate_workflow()?;
            if !r.complete() {
                return Err("SESSION_RECONCILIATION: record phiên mới chưa hoàn tất".into());
            }
        }
        if report
            .buffers
            .iter()
            .filter_map(|b| b.record.as_ref())
            .any(|r| r.record_seq < 1 || r.record_seq > c.total_count)
        {
            return Err("SESSION_RECONCILIATION: còn record ngoài prefix phiên mới".into());
        }
        let mut next = journal.clone();
        next.session_archives.push(SessionArchive {
            session_id: journal.session_id.clone(),
            next_session_id: self.config.confirmed_session_id.clone(),
            last_total: journal.last_total,
            ack: journal.ack.clone(),
            deliveries: journal.deliveries.clone(),
            warnings: journal.warnings.clone(),
            reason: change.reason.clone(),
            confirmed_at: Utc::now(),
        });
        next.previous_session_warnings
            .extend(next.warnings.drain(..));
        next.session_id = self.config.confirmed_session_id.clone();
        next.last_total = None;
        next.ack = Default::default();
        next.pause = None;
        next.session_request = None;
        Self::warn(
            &mut next,
            format!(
                "Đã đối chiếu phiên mới, source={}, session={:?}; dữ liệu/checkpoint phiên cũ giữ trong session_archives, lý do={}",
                self.config.source_key, self.config.confirmed_session_id, change.reason
            ),
        );
        Ok(next)
    }
    async fn reconcile_session(&self, report: &PollReport, pool: &PgPool) -> Result<(), String> {
        if self.config.session_transition.is_none() {
            return Err(
                "SESSION_RECONCILIATION: cần cấu hình đối chiếu phiên, không tự đổi session".into(),
            );
        }
        if let Some(intent) = &self.journal.lock().await.ack.intent {
            return Err(format!(
                "SESSION_RECONCILIATION: intent ACK phiên cũ target={} chưa giải quyết; giữ nguyên checkpoint",
                intent.target
            ));
        }
        if !self.storage_verified.load(Ordering::SeqCst) {
            return Err(
                "SESSION_RECONCILIATION: chờ xác minh receipt/checkpoint với PostgreSQL".into(),
            );
        }
        let (old_session, old_total) = {
            let journal = self.journal.lock().await;
            (
                journal.session_id.clone(),
                journal.last_total.unwrap_or(journal.ack.accepted),
            )
        };
        let (durable_prefix,): (i64,) = sqlx::query_as("SELECT count(DISTINCT i.record_seq) FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id WHERE s.source_key=$1 AND i.session_id IS NOT DISTINCT FROM $2 AND i.record_seq BETWEEN 1 AND $3")
            .bind(&self.config.source_key).bind(&old_session).bind(old_total).fetch_one(pool).await.map_err(|e| e.to_string())?;
        if durable_prefix != i64::from(old_total) {
            return Err(format!(
                "SESSION_RECONCILIATION: lịch sử prefix phiên cũ không đầy đủ: source={}, session={old_session:?}, expected={old_total}, durable={durable_prefix}",
                self.config.source_key
            ));
        }
        let candidate_session = self
            .config
            .confirmed_session_id
            .as_deref()
            .ok_or("Chưa chỉ định phiên mới")?;
        let (seen,): (bool,) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id WHERE s.source_key=$1 AND i.session_id=$2 UNION ALL SELECT 1 FROM measurements m JOIN plc_sources s ON s.id=m.source_id WHERE s.source_key=$1 AND m.session_id=$2)")
            .bind(&self.config.source_key).bind(candidate_session).fetch_one(pool).await.map_err(|e| e.to_string())?;
        let fresh = As200Reader::new(self.config.reader())
            .poll()
            .await
            .map_err(|e| e.to_string())?;
        if !report.control.same_buffer_state(&fresh.control)
            || report.control.pc_read_count != fresh.control.pc_read_count
            || report.control.pc_read_done != fresh.control.pc_read_done
            || report
                .buffers
                .iter()
                .zip(&fresh.buffers)
                .any(|(a, b)| a.raw_registers != b.raw_registers)
        {
            return Err("SESSION_RECONCILIATION: snapshot thay đổi trong lúc đối chiếu; chưa chấp nhận phiên".into());
        }
        let mut journal = self.journal.lock().await;
        if !self.storage_verified.load(Ordering::SeqCst) {
            return Err(
                "SESSION_RECONCILIATION: xác minh PostgreSQL thay đổi trong lúc đối chiếu".into(),
            );
        }
        let next = self.session_transition_candidate(&journal, &fresh, !seen)?;
        self.persist(&next)?; // Chỉ công bố phiên/checkpoint mới sau persist thành công.
        *journal = next;
        drop(journal);
        self.publish_journal().await?;
        let mut status = self.state.status.write().await;
        status.state = "WAITING".into();
        status.connection = "CONNECTED".into();
        status.message =
            "Đã đối chiếu phiên mới; tiếp tục theo dõi dữ liệu, dữ liệu phiên cũ được giữ nguyên."
                .into();
        Ok(())
    }
    fn matches(row: &InboxRecord, input: &InboxInput, source_id: i64) -> Result<(), String> {
        if row.source_id != source_id
            || row.record_seq != input.record_seq
            || row.raw_registers
                != serde_json::to_value(&input.raw_registers).map_err(|e| e.to_string())?
            || row.word_order != input.word_order.as_str()
            || (input.confirmed_session_id.is_some()
                && row.session_id != input.confirmed_session_id)
        {
            return Err("CONFLICT: Inbox đã có nhưng source/session/sequence/raw/WordOrder không khớp; không ghi đè hoặc đổi delivery ID.".into());
        }
        Ok(())
    }
    async fn row_status(&self, row: &InboxRecord) -> Result<(), String> {
        // Retry from an archived session must not replace the current session's display.
        if row.session_id != self.journal.lock().await.session_id {
            return Ok(());
        }
        let alias = self
            .journal
            .lock()
            .await
            .deliveries
            .iter()
            .find(|d| d.inbox_id == Some(row.id))
            .map(|d| d.input.ingest_key.clone());
        let mut status = self.state.status.write().await;
        let record = decode::record(
            &serde_json::from_value::<Vec<u16>>(row.raw_registers.clone())
                .map_err(|e| e.to_string())?,
            row.buffer_index as usize,
            match row.word_order.as_str() {
                "LOW" => super::as200::WordOrder::LowWordFirst,
                "HIGH" => super::as200::WordOrder::HighWordFirst,
                _ => return Err("WordOrder inbox không hợp lệ".into()),
            },
        )
        .map_err(|e| e.to_string())?;
        record.validate_metadata(row.total_count)?;
        if record.record_seq != row.record_seq {
            return Err("AS200_METADATA: inbox/raw sequence mismatch".into());
        }
        if !record.complete() {
            return Err(
                "Inbox chưa phải bản ghi hoàn thành; không tự xử lý thành measurement.".into(),
            );
        }
        let current = status.latest.as_ref().is_none_or(|current| {
            if row.session_id.is_some()
                && current.source_key == self.config.source_key
                && current.session_id == row.session_id
            {
                row.record_seq >= current.record_seq
            } else if current.session_id != row.session_id {
                true // Incoming session has already been checked against the accepted journal.
            } else {
                current.received_at <= row.received_at
                    || current.inbox_id == Some(row.id)
                    || alias.as_ref() == Some(&current.delivery_id)
            }
        });
        if !current {
            return Ok(());
        }
        if let Some(id) = row.measurement_id {
            status.last_saved_id = Some(id);
        }
        if current {
            status.latest = Some(LatestRecord {
                source_key: self.config.source_key.clone(),
                session_id: row.session_id.clone(),
                delivery_id: row.ingest_key.clone(),
                inbox_id: Some(row.id),
                measurement_id: row.measurement_id,
                record_seq: row.record_seq,
                total_count: row.total_count,
                product_code: record.product_code,
                club_number: record.stick_no,
                received_at: row.received_at,
                overall_result: format!("{:?}", record.overall_result).to_uppercase(),
                tip_result: format!("{:?}", record.tip.result).to_uppercase(),
                butt_result: format!("{:?}", record.butt.result).to_uppercase(),
                save_state: row.processing_status.clone(),
            });
        }
        status.state = row.processing_status.clone();
        status.message = match row.processing_status.as_str() {
            "PROCESSED" => format!(
                "Đã commit PostgreSQL. Mã bản ghi: {}",
                row.measurement_id.unwrap()
            ),
            "SESSION_UNCONFIRMED" => {
                "Đã giữ raw trong inbox; chưa xác nhận phiên, không tạo measurement.".into()
            }
            "FAILED" => row
                .error_message
                .clone()
                .unwrap_or_else(|| "Xử lý inbox thất bại; sẽ retry cùng inbox ID.".into()),
            _ => "Đã lưu inbox, đang chờ xử lý PostgreSQL.".into(),
        };
        Ok(())
    }
    async fn persist_receipts(&self, pool: &PgPool) -> Result<i64, String> {
        let source = SourceRepository::upsert(pool, &self.config.source_key, None).await?;
        let active_session = self.journal.lock().await.session_id.clone();
        if let Some(session) = &active_session {
            if let Some(previous) = InboxRepository::session_total(pool, source.id, session).await?
            {
                let mut journal = self.journal.lock().await;
                if journal
                    .last_total
                    .is_some_and(|current| i64::from(current) < previous)
                {
                    journal.pause = Some("RESET".into());
                    Self::warn(&mut journal,"RESET/CONFLICT: TotalCount thấp hơn lịch sử inbox trong cùng session. Receipt mới được cách ly SESSION_UNCONFIRMED; cần đối chiếu phiên.".into());
                    for delivery in &mut journal.deliveries {
                        if delivery.inbox_id.is_none() {
                            delivery.input.confirmed_session_id = None;
                        }
                    }
                }
            }
        }
        let deliveries = self.journal.lock().await.deliveries.clone();
        for delivery in &deliveries {
            let mut input = delivery.input.clone();
            input.source_id = source.id;
            let existing = if let Some(id) = delivery.inbox_id {
                let found = InboxRepository::find_optional(pool, id).await?;
                if found.is_none() {
                    let message = format!(
                        "LOCAL_HISTORY_MISSING: journal inbox ID {id} không còn trong database; không tự khôi phục hoặc ACK. Cần kỹ thuật đối chiếu database, cấu hình và phiên thiết bị."
                    );
                    let mut journal = self.journal.lock().await;
                    journal.pause = Some("LOCAL_HISTORY_MISSING".into());
                    Self::warn(&mut journal, message.clone());
                    drop(journal);
                    self.publish_journal().await?;
                    return Err(message);
                }
                found
            } else {
                InboxRepository::existing_delivery(pool, source.id, &input).await?
            };
            let row = if let Some(row) = existing {
                Self::matches(&row, &input, source.id)?;
                row
            } else {
                InboxService::receive(pool, &input).await?
            };
            {
                let mut journal = self.journal.lock().await;
                if let Some(item) = journal
                    .deliveries
                    .iter_mut()
                    .find(|d| d.input.ingest_key == input.ingest_key)
                {
                    item.inbox_id = Some(row.id);
                    item.terminal = row.processing_status == "PROCESSED"
                        || row.processing_status == "SESSION_UNCONFIRMED";
                }
            }
            if let Err(error) = self.row_status(&row).await {
                // Raw đã vào inbox, lỗi payload cũ không được chặn sink cho receipt khác.
                if row.session_id.is_some() && row.measurement_id.is_none() {
                    {
                        let mut journal = self.journal.lock().await;
                        if !journal
                            .retries
                            .iter()
                            .any(|e| e.inbox_id == row.id && e.source_id == row.source_id)
                        {
                            retry::reserve(
                                &mut journal.retries,
                                &row,
                                Utc::now(),
                                self.config.retry_interval_ms,
                                self.config.retry_max_delay_ms,
                            );
                            self.persist(&journal)?;
                        }
                    }
                    self.retry_failure(&row, &error, true).await?;
                    if row.processing_status != "FAILED" {
                        InboxRepository::failed(pool, row.id, &error).await?;
                    }
                } else {
                    let mut journal = self.journal.lock().await;
                    Self::warn(
                        &mut journal,
                        format!(
                            "INBOX_DIAGNOSTIC: source={}, inbox={}, session={:?}, status={}, error={error}. Giữ nguyên dữ liệu.",
                            row.source_id, row.id, row.session_id, row.processing_status
                        ),
                    );
                }
            }
        }
        // Inbox đã commit; checkpoint cache được sync một lần trước storage_verified=true.
        self.publish_journal().await?;
        Ok(source.id)
    }
    async fn retry_failure(
        &self,
        row: &InboxRecord,
        error: &str,
        permanent: bool,
    ) -> Result<(), String> {
        let mut journal = self.journal.lock().await;
        if let Some(entry) = journal
            .retries
            .iter_mut()
            .find(|e| e.inbox_id == row.id && e.source_id == row.source_id)
        {
            entry.blocked = permanent || entry.attempts >= self.config.retry_max_attempts;
            entry.error = error.to_string();
            let blocked = entry.blocked;
            if blocked {
                for d in &mut journal.deliveries {
                    if d.inbox_id == Some(row.id) {
                        d.terminal = true;
                    } // Raw vẫn ở inbox, không phải PROCESSED.
                }
                Self::warn(
                    &mut journal,
                    format!(
                        "INBOX_RETRY_BLOCKED: inbox={}, source={}, session={:?}. Dữ liệu được giữ; cần kỹ thuật kiểm tra rồi yêu cầu thử lại.",
                        row.id, row.source_id, row.session_id
                    ),
                );
            }
        }
        self.persist(&journal)
    }
    async fn process_pending(&self, pool: &PgPool, source_id: i64) -> Result<(), String> {
        self.process_pending_using(&retry::PostgresBackend(pool), source_id, Utc::now())
            .await
    }
    async fn process_pending_using<B: retry::InboxBackend>(
        &self,
        backend: &B,
        source_id: i64,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        let ids: Vec<i64> = self
            .journal
            .lock()
            .await
            .retries
            .iter()
            .filter(|e| e.source_id == source_id)
            .map(|e| e.inbox_id)
            .collect();
        if !ids.is_empty() {
            // Commit có thể đã thành công nhưng client mất response/readback.
            // Đối chiếu DB trước khi báo còn FAILED hoặc thử transaction lần nữa.
            for row in backend.completed(source_id, ids).await? {
                if row.source_id != source_id
                    || row.processing_status != "PROCESSED"
                    || row.measurement_id.is_none()
                    || row.processed_at.is_none()
                {
                    return Err("CONFLICT: retry readback không phải commit hợp lệ".into());
                }
                {
                    let journal = self.journal.lock().await;
                    if journal.retries.iter().any(|e| {
                        e.source_id == source_id
                            && e.inbox_id == row.id
                            && e.session_id != row.session_id
                    }) {
                        return Err("CONFLICT: phiên inbox đã commit không khớp lịch retry".into());
                    }
                }
                if let Err(error) = self.row_status(&row).await {
                    eprintln!("AS200 committed inbox diagnostic id={}: {error}", row.id);
                }
                let mut journal = self.journal.lock().await;
                journal
                    .retries
                    .retain(|e| !(e.source_id == source_id && e.inbox_id == row.id));
                for d in &mut journal.deliveries {
                    if d.inbox_id == Some(row.id) {
                        d.terminal = true;
                    }
                }
                self.persist(&journal)?;
            }
        }
        let excluded = retry::excluded(
            &self.journal.lock().await.retries,
            source_id,
            now,
            self.config.retry_max_attempts,
        );
        let rows = backend.candidates(source_id, excluded).await?;
        let mut errors = Vec::new();
        for row in rows {
            if row.source_id != source_id || row.session_id.is_none() {
                return Err(
                    "CONFLICT: retry backend trả dữ liệu ngoài nguồn/phiên được phép".into(),
                );
            }
            {
                let mut journal = self.journal.lock().await;
                retry::reserve(
                    &mut journal.retries,
                    &row,
                    now,
                    self.config.retry_interval_ms,
                    self.config.retry_max_delay_ms,
                );
                self.persist(&journal)?; // Reservation qua restart/timeout trước khi chạy transaction.
            }
            let validation =
                crate::services::inbox_service::draft_from_inbox(&row).and_then(|draft| {
                    if draft.overall_result == crate::enums::MeasurementResult::Unmeasured
                        || draft
                            .stations
                            .iter()
                            .any(|s| s.result == crate::enums::MeasurementResult::Unmeasured)
                    {
                        Err("Dữ liệu chưa hoàn tất; cần đối chiếu quy trình PLC".into())
                    } else {
                        Ok(())
                    }
                });
            if let Err(error) = validation {
                self.retry_failure(&row, &error, true).await?;
                backend.fail(row.id, &error).await?;
                errors.push(error);
                continue;
            }
            self.row_status(&row).await?;
            {
                let mut status = self.state.status.write().await;
                status.state = "SAVING".into();
                status.message = format!(
                    "Đang xử lý inbox {} và lưu transaction PostgreSQL...",
                    row.id
                );
            }
            match backend.process(row.id).await {
                Ok(updated) => {
                    self.row_status(&updated).await?;
                    let mut journal = self.journal.lock().await;
                    journal
                        .retries
                        .retain(|e| !(e.inbox_id == row.id && e.source_id == source_id));
                    for item in &mut journal.deliveries {
                        if item.inbox_id == Some(row.id) {
                            item.terminal = true;
                        }
                    }
                    self.persist(&journal)?;
                }
                Err(error) => {
                    // Không phân loại SQL bằng chuỗi lỗi: lỗi chưa rõ được backoff hữu hạn.
                    self.retry_failure(&row, &error, false).await?;
                    backend.fail(row.id, &error).await?;
                    let mut status = self.state.status.write().await;
                    if let Some(latest) = status
                        .latest
                        .as_mut()
                        .filter(|r| r.inbox_id == Some(row.id))
                    {
                        latest.save_state = "FAILED".into();
                    }
                    errors.push(error);
                }
            }
        }
        self.publish_journal().await?;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
    async fn queue_retry(&self, id: i64) -> Result<(), String> {
        let mut journal = self.journal.lock().await;
        let mut next = journal.clone();
        if next
            .manual_retries
            .iter()
            .filter(|r| matches!(r.state.as_str(), "PENDING" | "WAITING"))
            .count()
            >= 100
        {
            return Err("Đã có nhiều yêu cầu thử lại đang chờ".into());
        }
        next.manual_retries.retain(|r| r.inbox_id != id);
        next.manual_retries.push(ManualRetryRequest {
            inbox_id: id,
            state: "PENDING".into(),
            error: None,
        });
        self.persist(&next)?;
        *journal = next;
        Ok(())
    }
    async fn retry_result(
        &self,
        id: i64,
        state: &str,
        error: Option<String>,
    ) -> Result<(), String> {
        let mut journal = self.journal.lock().await;
        let mut next = journal.clone();
        if let Some(r) = next.manual_retries.iter_mut().find(|r| r.inbox_id == id) {
            r.state = state.into();
            r.error = error;
        }
        self.persist(&next)?;
        *journal = next;
        Ok(())
    }
    async fn apply_retry_requests(&self, pool: &PgPool, source_id: i64) -> Result<(), String> {
        self.apply_retry_requests_using(&retry::PostgresBackend(pool), source_id)
            .await
    }
    async fn apply_retry_requests_using(
        &self,
        backend: &impl retry::RetryLookup,
        source_id: i64,
    ) -> Result<(), String> {
        let requests: Vec<i64> = self
            .journal
            .lock()
            .await
            .manual_retries
            .iter()
            .filter(|r| matches!(r.state.as_str(), "PENDING" | "WAITING"))
            .map(|r| r.inbox_id)
            .collect();
        for id in requests {
            match backend.find(id).await {
                Err(error) => self.retry_result(id, "WAITING", Some(error)).await?,
                Ok(None) => {
                    self.retry_result(id, "REJECTED", Some("Không tìm thấy inbox".into()))
                        .await?
                }
                Ok(Some(row)) if row.source_id != source_id || row.session_id.is_none() => {
                    self.retry_result(
                        id,
                        "REJECTED",
                        Some("Nguồn hoặc phiên chưa được xác nhận".into()),
                    )
                    .await?;
                }
                Ok(Some(row)) => match self.apply_retry_row(&row, source_id).await {
                    Ok(()) => self.retry_result(id, "APPLIED", None).await?,
                    Err(error) => self.retry_result(id, "WAITING", Some(error)).await?,
                },
            }
        }
        self.publish_journal().await
    }
    async fn apply_retry_row(&self, row: &InboxRecord, source_id: i64) -> Result<(), String> {
        if row.source_id != source_id || row.session_id.is_none() {
            return Err("Không thử lại inbox ngoài nguồn hoặc chưa xác nhận phiên".into());
        }
        if row.processing_status == "PROCESSED" {
            self.row_status(row).await?;
        }
        let mut journal = self.journal.lock().await;
        let mut next = journal.clone();
        next.retries
            .retain(|e| !(e.inbox_id == row.id && e.source_id == source_id));
        Self::warn(
            &mut next,
            format!(
                "MANUAL_RETRY: source={}, inbox={}, session={:?}, lúc {}. Không thay raw/identity/checkpoint.",
                source_id,
                row.id,
                row.session_id,
                Utc::now()
            ),
        );
        self.persist(&next)?;
        *journal = next;
        Ok(())
    }
    async fn flush(&self, pool: &PgPool) -> Result<(), String> {
        self.storage_verified.store(false, Ordering::SeqCst);
        let source = self.persist_receipts(pool).await?;
        self.storage_verified.store(true, Ordering::SeqCst);
        self.apply_retry_requests(pool, source).await?;
        self.process_pending(pool, source).await
    }
    async fn apply_gap_ack(&self) -> Result<(), String> {
        let mut journal = self.journal.lock().await;
        if !matches!(journal.pause.as_deref(), Some("OVERFLOW" | "QUEUE_FULL")) {
            return Err("Reset/xung đột phiên không thể bỏ qua bằng xác nhận buffer gap".into());
        }
        journal.pause = None;
        Self::warn(
            &mut journal,
            format!(
                "Người dùng xác nhận đã kiểm tra buffer gap lúc {}. Không khôi phục hay dựng các record bị mất.",
                Utc::now()
            ),
        );
        drop(journal);
        self.publish_journal().await?;
        let mut status = self.state.status.write().await;
        status.state = "WAITING".into();
        status.message =
            "Đã xác nhận buffer gap, tiếp tục theo dõi. Cảnh báo lịch sử vẫn được giữ.".into();
        Ok(())
    }
    async fn reader_loop(self: Arc<Self>, database: DatabaseState) {
        let mut reader = As200Reader::new(self.config.reader());
        loop {
            if self.state.stopping.load(Ordering::SeqCst) { break; }
            let reconnect_waiters = std::mem::take(&mut *self.state.reconnect_waiters.lock().await);
            if self.state.ack_gap.swap(false, Ordering::SeqCst) {
                if let Err(e) = self.apply_gap_ack().await {
                    self.error(e).await;
                }
            }
            let (paused, reconciling) = {
                let journal = self.journal.lock().await;
                (journal.pause.is_some(), journal.pause.as_deref() == Some("SESSION_RECONCILIATION"))
            };
            if reconciling {
                let result = async {
                    let snapshot = As200Reader::new(self.config.reader())
                        .poll()
                        .await
                        .map_err(|e| e.to_string())?;
                    let pool = database.pool().await?;
                    self.reconcile_session(&snapshot, &pool).await
                }
                .await;
                match result {
                    Ok(()) => reader = As200Reader::new(self.config.reader()),
                    Err(e) => self.error(e).await,
                }
            }
            let reconnect_result = if !paused {
                match reader.poll().await {
                    Ok(report) => {
                        if self.state.stopping.load(Ordering::SeqCst) { break; }
                        let diagnostics_clear = report.diagnostics.is_empty();
                        if let Err(e) = self.capture(report.clone()).await {
                            self.error(e.clone()).await;
                            Err(e)
                        } else if self.config.ack_enabled {
                            if let Err(error) = self.ack_step(&report).await {
                                eprintln!("AS200 ACK failed: {error}");
                                self.state.status.write().await.ack_status = "FAILED";
                                Err(error)
                            } else if diagnostics_clear && {
                                let status = self.state.status.read().await;
                                status.pause_reason.is_none()
                                    && status.connection == "CONNECTED"
                                    && !matches!(status.ack_status, "FAILED" | "RECONCILIATION_REQUIRED")
                            } {
                                Ok(())
                            } else {
                                Err("PLC đã trả dữ liệu cần đối chiếu; chưa thể xác nhận tiếp tục thu an toàn.".into())
                            }
                        } else if diagnostics_clear && {
                            let status = self.state.status.read().await;
                            status.pause_reason.is_none() && status.connection == "CONNECTED"
                        } {
                            Ok(())
                        } else {
                            Err("PLC đã trả dữ liệu cần đối chiếu; chưa thể xác nhận tiếp tục thu an toàn.".into())
                        }
                    }
                    Err(e) => {
                        let message = e.to_string();
                        self.read_error(e).await;
                        Err(message)
                    }
                }
            } else {
                Err("Worker đang tạm dừng để bảo vệ dữ liệu; cần đối chiếu trước khi kết nối lại.".into())
            };
            for waiter in reconnect_waiters {
                let _ = waiter.send(reconnect_result.clone());
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(self.config.poll_interval_ms)) => {},
                _ = self.state.reconnect_now.notified() => {},
            }
        }
    }
    async fn heartbeat_loop(self: Arc<Self>) {
        use super::as200::{ack::HeartbeatWriter, heartbeat::HeartbeatMonitor};
        let writer = if self.config.pc_heartbeat_enabled {
            HeartbeatWriter::new(
                self.config.reader(),
                &self.config.mode,
                true,
                self.config.allow_plc_connection,
                self.config.allow_plc_heartbeat_write,
            )
            .ok()
        } else {
            None
        };
        let mut reader = As200Reader::new(self.config.reader());
        *self.state.heartbeat.lock().unwrap() = Some(HeartbeatMonitor::new(
            self.config.heartbeat_timeout(),
            std::time::Instant::now(),
        ));
        let mut counter = None;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if self.state.stopping.load(Ordering::SeqCst) { break; }
            tick.tick().await;
            if self.state.stopping.load(Ordering::SeqCst) { break; }
            match reader.read_control().await {
                Ok(control) => {
                    let health = self
                        .state
                        .heartbeat
                        .lock()
                        .unwrap()
                        .as_mut()
                        .unwrap()
                        .observe(control.heartbeat, std::time::Instant::now());
                    self.state.status.write().await.plc_health = health.into();
                    if let Some(writer) = &writer {
                        if control.pc_heartbeat < 0 {
                            self.state.status.write().await.pc_heartbeat_status = "FAILED".into();
                            continue;
                        }
                        let last = *counter.get_or_insert(control.pc_heartbeat as u16);
                        let next = super::as200::heartbeat::next_pc_heartbeat(last)
                            .expect("counter đã kiểm tra phạm vi");
                        match writer.write(next).await {
                            Ok(()) => {
                                counter = Some(next);
                                self.state.status.write().await.pc_heartbeat_status = "SENT".into();
                            }
                            Err(error) => {
                                eprintln!("AS200 PC heartbeat: {error}");
                                self.state.status.write().await.pc_heartbeat_status =
                                    "FAILED".into();
                            }
                        }
                    }
                }
                Err(error) => {
                    eprintln!("AS200 heartbeat monitor: {error}");
                    self.state.status.write().await.plc_health = "DISCONNECTED".into();
                    if writer.is_some() {
                        self.state.status.write().await.pc_heartbeat_status = "FAILED".into();
                    }
                }
            }
        }
    }
    async fn sink_loop(self: Arc<Self>, database: DatabaseState) {
        loop {
            if self.state.stopping.load(Ordering::SeqCst) { break; }
            let result = tokio::time::timeout(Duration::from_secs(8), async {
                let pool = database.pool().await?;
                self.flush(&pool).await
            })
            .await;
            let error=match result{Ok(Ok(()))=>None,Ok(Err(e))=>Some(e),Err(_)=>Some("Hết thời gian PostgreSQL; retry giữ nguyên delivery/inbox ID, không báo thành công trước commit.".into())};
            if let Some(error) = error {
                let mut journal = self.journal.lock().await;
                if error.contains("CONFLICT") {
                    journal.pause = Some("CONFLICT".into());
                    Self::warn(&mut journal, error.clone());
                }
                drop(journal);
                let _ = self.publish_journal().await;
                self.error(error).await;
            }
            tokio::time::sleep(Duration::from_millis(self.config.retry_interval_ms)).await;
        }
    }
}
impl Engine {
    // A pending read, retry or temporary loss of storage/heartbeat verification is
    // not evidence that a previously reported ACK failure has been resolved.
    async fn ack_progress_status(&self, next: &'static str) {
        let mut status = self.state.status.write().await;
        if !matches!(status.ack_status, "FAILED" | "RECONCILIATION_REQUIRED") {
            status.ack_status = next;
        }
    }

    async fn ack_step(&self, report: &PollReport) -> Result<(), String> {
        if !self.state.fresh_health().await {
            // Heartbeat freshness gates new ACK writes, but an already accepted
            // checkpoint can still be confirmed by this read-only snapshot.
            if self.storage_verified.load(Ordering::SeqCst) && report.diagnostics.is_empty() {
                let journal = self.journal.lock().await;
                let mut binding = format!(
                    "{}|{}|{}|{}|{:?}|{}|{:?}",
                    self.config.mode,
                    self.config.address,
                    self.config.unit_id,
                    self.config.source_key,
                    self.config.confirmed_session_id,
                    self.config.modbus_address_offset,
                    self.config.word_order
                );
                if self.config.ack_protocol.as_deref() == Some("as200") {
                    binding.push_str("|as200");
                }
                let checkpoint = journal.ack.accepted;
                if journal.pause.is_none()
                    && journal.session_id == self.config.confirmed_session_id
                    && journal.ack.binding == binding
                    && journal.ack.intent.is_none()
                    && checkpoint > 0
                    && report.control.total_count == checkpoint
                    && report.control.pc_read_count == checkpoint
                    && report.control.pc_read_done == 0
                    && report.control.data_ready == 0
                {
                    self.state.status.write().await.ack_status = "ACKNOWLEDGED";
                    return Ok(());
                }
            }
            self.ack_progress_status("WAITING").await;
            return Ok(());
        }
        for record in report.buffers.iter().filter_map(|b| b.record.as_ref()) {
            record.validate_metadata(Some(i64::from(report.control.total_count)))?;
            record.validate_workflow()?;
        }
        if !self.storage_verified.load(Ordering::SeqCst) {
            self.ack_progress_status("WAITING").await;
            return Ok(());
        }
        use super::as200::ack::{AckIntent, AckWriter, accepted, accepted_as200, same_records};
        let writer = AckWriter::new(
            self.config.reader(),
            &self.config.mode,
            self.config.ack_enabled,
            self.config.ack_protocol.as_deref(),
            self.config.allow_plc_connection,
        )?;
        let mut journal = self.journal.lock().await;
        if journal.session_id != self.config.confirmed_session_id {
            return Err("ACK bị chặn: phiên cấu hình chưa được đối chiếu với journal".into());
        }
        if journal.pause.is_some() || !report.diagnostics.is_empty() {
            self.ack_progress_status("WAITING").await;
            return Ok(());
        }
        let mut binding = format!(
            "{}|{}|{}|{}|{:?}|{}|{:?}",
            self.config.mode,
            self.config.address,
            self.config.unit_id,
            self.config.source_key,
            self.config.confirmed_session_id,
            self.config.modbus_address_offset,
            self.config.word_order
        );
        if self.config.ack_protocol.as_deref() == Some("as200") {
            binding.push_str("|as200");
        }
        if journal.ack.binding.is_empty() {
            journal.ack.binding = binding.clone();
            self.persist(&journal)?;
        }
        if journal.ack.binding != binding {
            return Err("ACK source/session/transport binding mismatch".into());
        }
        let as200 = self.config.ack_protocol.as_deref() == Some("as200");
        // Intent từ phiên bản cũ cũng phải qua validation chung, không chỉ receipt mới.
        if let Some(intent) = &journal.ack.intent {
            for seq in journal.ack.accepted + 1..=intent.target {
                let receipt = journal
                    .deliveries
                    .iter()
                    .find(|d| {
                        d.input.confirmed_session_id == self.config.confirmed_session_id
                            && d.input.record_seq == seq
                            && d.inbox_id.is_some()
                    })
                    .ok_or("ACK intent thiếu receipt bền vững; cần đối chiếu")?;
                let record = decode::record(
                    &receipt.input.raw_registers,
                    receipt.input.buffer_index as usize,
                    self.config.reader().word_order,
                )
                .map_err(|e| e.to_string())?;
                record.validate_metadata(receipt.input.total_count)?;
                if record.record_seq != seq {
                    return Err("ACK intent: RecordSeq không khớp receipt; cần đối chiếu".into());
                }
                record.validate_workflow()?;
                if !record.complete() {
                    return Err("ACK intent chưa hoàn tất".into());
                }
            }
        }
        let is_accepted = |intent: &AckIntent, snapshot: &PollReport| {
            if as200 {
                accepted_as200(intent, snapshot)
            } else {
                accepted(intent, snapshot)
            }
        };
        if let Some(mut intent) = journal.ack.intent.clone() {
            if self.config.mode == "simulator"
                && intent.submitted_at_ms.is_none()
                && report.control.total_count > intent.target
                && report.control.pc_read_count == journal.ack.accepted
                && report.control.pc_read_done == 0
                && same_records(&intent, report)
            {
                // Chưa có Count/Done nào áp dụng: nhận đủ snapshot mới rồi chuẩn bị lại target.
                // Nếu Count đã ghi nhưng mất response, PC_ReadCount khác accepted nên không vào đây.
                journal.ack.intent = None;
                self.persist(&journal)?;
                self.ack_progress_status("WAITING").await;
                return Ok(());
            }
            if as200
                && intent.submitted_at_ms.is_some()
                && report.control.pc_read_done == 1
                && report.control.pc_read_count == intent.target
            {
                intent.observed_done_high = true;
                journal.ack.intent = Some(intent.clone());
                self.persist(&journal)?;
            }
            if is_accepted(&intent, report) {
                journal.ack.accepted = intent.target;
                journal.ack.intent = None;
                self.persist(&journal)?;
                self.state.status.write().await.ack_status =
                    if report.control.total_count == intent.target {
                        "ACKNOWLEDGED"
                    } else {
                        "WAITING"
                    };
                return Ok(());
            }
            if report.control.pc_read_done == 1 {
                // Không lặp xung khi chưa biết PLC đã áp dụng lệnh trước hay chưa.
                if intent.submitted_at_ms.is_some_and(|at| {
                    Utc::now().timestamp_millis() - at < self.config.timeout_ms as i64
                }) {
                    self.ack_progress_status("WAITING").await;
                } else {
                    self.state.status.write().await.ack_status = "FAILED";
                }
                return Ok(());
            }
            if as200 && intent.phase() != super::as200::ack::AckPhase::NotSent {
                // Không thấy đủ cạnh 1->0 sau restart/mất phản hồi: chỉ quan sát, không phát lại Done.
                self.state.status.write().await.ack_status =
                    if intent.submitted_at_ms.is_some_and(|at| {
                        Utc::now().timestamp_millis() - at < self.config.timeout_ms as i64
                    }) {
                        "WAITING"
                    } else {
                        "RECONCILIATION_REQUIRED"
                    };
                self.state.status.write().await.message =
                    "ACK cần đối chiếu: chưa đủ bằng chứng PLC đã xác nhận. Giữ nguyên dữ liệu và yêu cầu đang chờ; không tự gửi lại tín hiệu.".into();
                return Ok(());
            }
            if !same_records(&intent, report)
                || report.control.total_count < intent.target
                || (!as200 && report.control.total_count != intent.target)
            {
                return Err("ACK pending buffer changed; manual reconciliation required".into());
            }
            let fresh = writer.verify_snapshot(&intent).await?;
            if is_accepted(&intent, &fresh) {
                // Chỉ poll tiếp; lần sau sẽ lưu checkpoint đã xác nhận.
                self.ack_progress_status("WAITING").await;
                return Ok(());
            }
            if fresh.control.total_count < intent.target
                || (!as200 && fresh.control.total_count != intent.target)
                || fresh.control.data_ready != 1
                || fresh.control.pc_read_done != 0
                || ![journal.ack.accepted, intent.target].contains(&fresh.control.pc_read_count)
            {
                return Err("ACK control changed/refused".into());
            }
            self.ack_progress_status("PENDING").await;
            if !self.state.fresh_health().await {
                return Ok(());
            }
            writer.count(intent.target).await?;
            let checked = writer.verify_snapshot(&intent).await?;
            if checked.control.total_count < intent.target
                || (!as200 && checked.control.total_count != intent.target)
                || checked.control.pc_read_count != intent.target
                || checked.control.pc_read_done != 0
                || checked.control.data_ready != 1
            {
                return Err("ACK control changed after count write".into());
            }
            let prepared = writer.prepare_done().await?;
            if !self.state.fresh_health().await {
                return Ok(());
            }
            journal.ack.intent.as_mut().unwrap().submitted_at_ms =
                Some(Utc::now().timestamp_millis());
            self.persist(&journal)?;
            // Crash sau persist/trước write vẫn là kết quả không rõ: không thể
            // giải quyết atomicity giữa ổ đĩa và socket bằng một flag phần mềm.
            writer.done_prepared(prepared).await?;
            self.ack_progress_status("WAITING").await;
            return Ok(());
        }
        if report.control.total_count == journal.ack.accepted && journal.ack.accepted > 0 {
            if report.control.pc_read_count != journal.ack.accepted
                || report.control.pc_read_done != 0
                || report.control.data_ready != 0
            {
                return Err("ACK accepted checkpoint no longer matches remote control".into());
            }
            self.state.status.write().await.ack_status = "ACKNOWLEDGED";
            return Ok(());
        }
        if report.control.pc_read_count != journal.ack.accepted || report.control.pc_read_done != 0
        {
            return Err("ACK unknown remote checkpoint; refusing to skip records".into());
        }
        let target = report.control.total_count;
        if target < journal.ack.accepted || (!as200 && target - journal.ack.accepted > 2) {
            return Err("ACK sequence gap/reset".into());
        }
        if target == 0 {
            // Empty baseline with no pending intent and a matching remote checkpoint.
            if report.control.data_ready == 0 {
                self.state.status.write().await.ack_status = "WAITING";
            } else {
                self.ack_progress_status("WAITING").await;
            }
            return Ok(());
        }
        self.ack_progress_status("WAITING").await;
        if report.control.data_ready != 1 {
            return Ok(());
        }
        if i64::from(target) - i64::from(journal.ack.accepted) > journal.deliveries.len() as i64 {
            return Err("ACK missing durable consecutive receipt/count mapping".into());
        }
        let mut records = Vec::new();
        for seq in journal.ack.accepted + 1..=target {
            let receipt = journal.deliveries.iter().find(|d| {
                d.input.confirmed_session_id == self.config.confirmed_session_id
                    && d.input.record_seq == seq
            });
            let Some(receipt) = receipt else {
                return Err("ACK missing sequence/count mapping".into());
            };
            let decoded = decode::record(
                &receipt.input.raw_registers,
                receipt.input.buffer_index as usize,
                match receipt.input.word_order {
                    crate::models::StoredWordOrder::Low => super::as200::WordOrder::LowWordFirst,
                    crate::models::StoredWordOrder::High => super::as200::WordOrder::HighWordFirst,
                },
            )
            .map_err(|e| e.to_string())?;
            decoded.validate_metadata(receipt.input.total_count)?;
            decoded.validate_workflow()?;
            if !decoded.complete() || decoded.record_seq != seq {
                return Err("ACK receipt chưa hợp lệ/hoàn tất; cần đối chiếu".into());
            }
            if receipt.inbox_id.is_none() {
                return Ok(());
            }
            if let Some(buffer) = report.buffers.iter().find(|b| {
                b.record
                    .as_ref()
                    .is_some_and(|r| r.record_seq == seq && r.complete())
            }) {
                if buffer.raw_registers != receipt.input.raw_registers {
                    return Err("ACK raw mismatch".into());
                }
                records.push((buffer.buffer_index, buffer.raw_registers.clone()));
            } else if !as200 {
                return Err("ACK missing sequence in stable buffers".into());
            }
        }
        if records.is_empty() {
            return Err("ACK no stable current record for target".into());
        }
        journal.ack.intent = Some(AckIntent {
            target,
            records,
            submitted_at_ms: None,
            observed_done_high: false,
        });
        self.persist(&journal)?; // Ý định bền vững trước mọi lệnh mạng; không có DB transaction đang mở.
        self.ack_progress_status("PENDING").await;
        Ok(())
    }
}
impl WorkerState {
    pub(super) async fn verify_source_closed(&self, file: &std::path::Path, config: &SimulatorConfig, pool: &PgPool, for_reset: bool, trace: &str) -> Result<File, String> {
        let enabled = self.status.read().await.enabled;
        let task_count = self.tasks.lock().await.len();
        switch_log(trace, "WORKER_STOP_CHECK", if enabled || task_count != 0 { "WORKER_RUNNING" } else { "PASS" }, format!("expected_enabled=false actual_enabled={enabled} expected_tasks=0 actual_tasks={task_count}"));
        if self.status.read().await.enabled || !self.tasks.lock().await.is_empty() {
            return Err("Chưa dừng xong thu dữ liệu.".into());
        }
        let path = config.journal_file(file);
        let lock = OpenOptions::new().read(true).write(true).open(path.with_extension("lock"))
            .map_err(|error| {
                switch_log(trace, "JOURNAL_CHECK", "JOURNAL_LOCK_UNAVAILABLE", format!("open_error={error}"));
                "Không thể xác minh trạng thái máy đo cũ.".to_string()
            })?;
        lock.try_lock().map_err(|error| {
            switch_log(trace, "JOURNAL_CHECK", "JOURNAL_LOCK_HELD", format!("lock_error={error}"));
            "Máy đo cũ vẫn đang được sử dụng.".to_string()
        })?;
        let journal: Journal = serde_json::from_str(&std::fs::read_to_string(&path)
            .map_err(|error| {
                switch_log(trace, "JOURNAL_CHECK", "JOURNAL_READ_FAILED", format!("read_error={error}"));
                "Thiếu lịch sử máy đo cũ. Vui lòng liên hệ kỹ thuật.".to_string()
            })?)
            .map_err(|error| {
                switch_log(trace, "JOURNAL_CHECK", "JOURNAL_PARSE_FAILED", format!("parse_error={error}"));
                "Không thể đọc lịch sử máy đo cũ.".to_string()
            })?;
        switch_log(trace, "JOURNAL_CHECK", "OBSERVED", format!("version={} source_matches={} session_matches={} last_total={:?} deliveries={} quarantine={} pause_present={} retries={}", journal.version, journal.source_key == config.source_key, journal.session_id == config.confirmed_session_id, journal.last_total, journal.deliveries.len(), journal.quarantine.len(), journal.pause.is_some(), journal.retries.len()));
        for quarantine in &journal.quarantine {
            switch_log(
                trace,
                "QUARANTINE_CHECK",
                "OBSERVED",
                format!(
                    "buffer={} record_seq={:?} total={} error={} received_at={} raw_words={} nonzero_words={} raw_checksum={} control={:?}",
                    quarantine.buffer_index,
                    quarantine.record_seq,
                    quarantine.total_count,
                    quarantine.error,
                    quarantine.received_at,
                    quarantine.raw_registers.len(),
                    quarantine.raw_registers.iter().filter(|word| **word != 0).count(),
                    quarantine.raw_registers.iter().fold(0u32, |sum, word| {
                        sum.wrapping_add(u32::from(*word))
                    }),
                    quarantine.control.as_ref().map(|control| (
                        control.total_count,
                        control.pc_read_count,
                        control.pc_read_done,
                        control.data_ready
                    ))
                ),
            );
        }
        let busy = "Máy đo cũ còn dữ liệu chưa xử lý. Chưa thể chuyển kết nối.";
        if !matches!(journal.version, 1 | 2) || journal.source_key != config.source_key
            || journal.session_id != config.confirmed_session_id || journal.session_id.is_none() {
            switch_log(trace, "JOURNAL_CHECK", "SOURCE_IDENTITY_UNVERIFIED", "expected=matching_source_session_and_supported_version actual=mismatch_or_missing");
            return Err("Không thể xác minh lịch sử máy đo cũ. Vui lòng liên hệ kỹ thuật.".into());
        }
        if journal.ack.intent.is_some() {
            switch_log(trace, "ACK_CHECK", "ACK_INTENT_PENDING", format!("accepted={} intent_present=true", journal.ack.accepted));
            return Err("Máy đo cũ còn ACK chưa có bằng chứng hoàn tất. Chưa thể chuyển kết nối.".into());
        }
        let empty_reset = for_reset && journal.has_reconciliable_reset_diagnostics(config);
        if journal.pause.as_deref() == Some("RESET")
            && (journal
                .last_total
                .is_some_and(|total| journal.ack.accepted < total)
                || journal.deliveries.iter().any(|delivery| !delivery.terminal)
                || (!journal.quarantine.is_empty() && !empty_reset))
        {
            switch_log(
                trace,
                "RESET_CHECK",
                "DATA_LOSS_RISK",
                format!(
                    "last_total={:?} accepted={} pending_deliveries={} quarantine={} empty_reset={}",
                    journal.last_total,
                    journal.ack.accepted,
                    journal.deliveries.iter().filter(|delivery| !delivery.terminal).count(),
                    journal.quarantine.len(),
                    empty_reset
                ),
            );
            return Err(
                "PLC đã reset khi còn bản ghi chưa được lưu/ACK đầy đủ; có nguy cơ mất dữ liệu. Không tự tạo epoch mới."
                    .into(),
            );
        }
        if (!empty_reset && (journal.pause.is_some() || !journal.quarantine.is_empty()))
            || journal.session_request.is_some() || config.session_transition.is_some() {
            switch_log(trace, "JOURNAL_CHECK", "DIAGNOSTIC_PENDING", format!("pause_present={} quarantine={} session_request={} transition={}", journal.pause.is_some(), journal.quarantine.len(), journal.session_request.is_some(), config.session_transition.is_some()));
            return Err("Máy đo cũ còn cảnh báo hoặc trạng thái cần đối chiếu. Chưa thể chuyển kết nối.".into());
        }
        if !journal.retries.is_empty()
            || journal.manual_retries.iter().any(|r| matches!(r.state.as_str(), "PENDING" | "WAITING")) {
            switch_log(trace, "JOURNAL_CHECK", "RETRY_PENDING", format!("retry_entries={} manual_pending={}", journal.retries.len(), journal.manual_retries.iter().filter(|r| matches!(r.state.as_str(), "PENDING" | "WAITING")).count()));
            return Err(busy.into());
        }
        let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id WHERE s.source_key=$1 AND (i.processing_status <> 'PROCESSED' OR i.measurement_id IS NULL OR i.processed_at IS NULL)")
            .bind(&config.source_key).fetch_one(pool).await.map_err(|_| "Không thể đối chiếu dữ liệu máy đo cũ.".to_string())?;
        switch_log(trace, "INBOX_CHECK", if pending == 0 { "PASS" } else { "INBOX_PENDING" }, format!("expected_pending=0 actual_pending={pending}"));
        if pending != 0 { return Err(busy.into()); }
        if let Some(total) = journal.last_total.filter(|n| *n > 0) {
            let last: Option<i32> = sqlx::query_scalar("SELECT max(m.record_seq) FROM measurements m JOIN plc_sources s ON s.id=m.source_id WHERE s.source_key=$1 AND m.session_id=$2")
                .bind(&config.source_key).bind(&config.confirmed_session_id).fetch_one(pool).await.map_err(|_| busy.to_string())?;
            switch_log(trace, "INBOX_CHECK", if last == Some(total) { "PASS" } else { "MEASUREMENT_CHECKPOINT_MISMATCH" }, format!("expected_last_seq={total} actual_last_seq={last:?}"));
            if last != Some(total) { return Err("Lịch sử máy đo cũ không khớp cơ sở dữ liệu. Chưa thể chuyển kết nối.".into()); }
            if empty_reset {
                let processed_prefix: i64 = sqlx::query_scalar("SELECT count(*) FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id JOIN measurements m ON m.id=i.measurement_id AND m.source_id=i.source_id AND m.session_id=i.session_id AND m.record_seq=i.record_seq WHERE s.source_key=$1 AND i.session_id=$2 AND i.record_seq BETWEEN 1 AND $3 AND i.processing_status='PROCESSED' AND i.processed_at IS NOT NULL")
                    .bind(&config.source_key).bind(&config.confirmed_session_id).bind(total)
                    .fetch_one(pool).await.map_err(|_| busy.to_string())?;
                if processed_prefix != i64::from(total) { return Err(busy.into()); }
            }
        }
        for delivery in &journal.deliveries {
            let id = delivery.inbox_id.ok_or(busy)?;
            let row = InboxRepository::find(pool, id).await.map_err(|_| busy.to_string())?;
            let key: String = sqlx::query_scalar("SELECT source_key FROM plc_sources WHERE id=$1")
                .bind(row.source_id).fetch_one(pool).await.map_err(|_| busy.to_string())?;
            if key != config.source_key || !processed_receipt_matches(&row, &delivery.input) {
                switch_log(trace, "INBOX_CHECK", "RECEIPT_MISMATCH", format!("record_seq={} expected=processed_matching_receipt actual=mismatch", delivery.input.record_seq));
                return Err(busy.into());
            }
        }
        switch_log(trace, "ACK_CHECK", "PASS", format!("accepted={} intent_present=false", journal.ack.accepted));
        Ok(lock)
    }

    pub(super) async fn initialize_separate_journal(config: SimulatorConfig, path: PathBuf) -> Result<(), String> {
        if path.exists() || path.with_extension("lock").exists() {
            return Err("Không thể tạo vùng tiếp nhận riêng.".into());
        }
        let engine = Engine::open(config, path, WorkerState::default())?;
        let mut journal = engine.journal.lock().await;
        // Caller proved an empty TotalCount=0 snapshot; retain that baseline to detect startup gaps.
        journal.last_total = Some(0);
        engine.persist(&journal)
    }

    /// Returns true only for a closed old session with a decreased counter and clean
    /// baseline, eligible for operator confirmation; never authorizes a reset itself.
    pub(super) async fn verify_resume(&self, file: &std::path::Path, config: &SimulatorConfig, historical: bool, database: &DatabaseState, trace: &str) -> Result<bool, String> {
        let path = config.journal_file(file);
        let journal: Journal = serde_json::from_str(&std::fs::read_to_string(path)
            .map_err(|error| {
                switch_log(trace, "JOURNAL_CHECK", "JOURNAL_READ_FAILED", format!("read_error={error}"));
                "Thiếu lịch sử máy đo. Vui lòng liên hệ kỹ thuật."
            })?)
            .map_err(|error| {
                switch_log(trace, "JOURNAL_CHECK", "JOURNAL_PARSE_FAILED", format!("parse_error={error}"));
                "Không thể xác minh lịch sử máy đo. Vui lòng liên hệ kỹ thuật."
            })?;
        if journal.ack.intent.is_some() {
            switch_log(trace, "ACK_CHECK", "ACK_INTENT_PENDING", format!("accepted={} intent_present=true", journal.ack.accepted));
            return Err("Máy đo cũ còn ACK chưa hoàn tất. Vui lòng liên hệ kỹ thuật đối chiếu xác nhận đã đọc trước khi bắt đầu phiên mới.".into());
        }
        let empty_reset = journal.has_reconciliable_reset_diagnostics(config);
        if !matches!(journal.version, 1 | 2) || journal.source_key != config.source_key
            || journal.session_id != config.confirmed_session_id
            || (!empty_reset && (journal.pause.is_some() || !journal.quarantine.is_empty()))
            || journal.session_request.is_some() || config.session_transition.is_some() {
            let identity_valid = matches!(journal.version, 1 | 2) && journal.source_key == config.source_key && journal.session_id == config.confirmed_session_id;
            switch_log(trace, "JOURNAL_CHECK", if identity_valid { "DIAGNOSTIC_PENDING" } else { "SOURCE_IDENTITY_UNVERIFIED" }, format!("version={} source_matches={} session_matches={} pause_present={} quarantine={} session_request={} transition={}", journal.version, journal.source_key == config.source_key, journal.session_id == config.confirmed_session_id, journal.pause.is_some(), journal.quarantine.len(), journal.session_request.is_some(), config.session_transition.is_some()));
            return Err(format!("Máy đo cũ chưa thể kết nối lại: pause={}, quarantine={}, ACK đang chờ={}. Cần đối chiếu lịch sử/phiên trước khi tiếp tục.",
                journal.pause.as_deref().unwrap_or("không"), journal.quarantine.len(), journal.ack.intent.is_some()));
        }
        let report = As200Reader::new(config.reader()).poll().await.map_err(|error| {
            switch_log(trace, "PLC_SNAPSHOT", "PLC_READ_FAILED", format!("reader_error={error}"));
            "Không thể xác minh dữ liệu máy đo."
        })?;
        {
            let confirm = As200Reader::new(config.reader()).poll().await.map_err(|error| {
                switch_log(trace, "PLC_SNAPSHOT", "PLC_READ_FAILED", format!("second_reader_error={error}"));
                "Không thể đọc lại máy đo để xác minh trạng thái ổn định."
            })?;
            if !report.control.same_buffer_state(&confirm.control)
                || report.control.pc_read_count != confirm.control.pc_read_count
                || report.control.pc_read_done != confirm.control.pc_read_done
                || report.control.reserve != confirm.control.reserve
                || report.buffers.len() != confirm.buffers.len()
                || report.buffers.iter().zip(&confirm.buffers).any(|(a, b)|
                    a.buffer_index != b.buffer_index || a.raw_registers != b.raw_registers)
                || !confirm.diagnostics.is_empty()
            {
                switch_log(trace, "PLC_SNAPSHOT", "SNAPSHOT_CHANGED", format!(
                    "first_total={} second_total={} first_read_count={} second_read_count={} first_done={} second_done={} first_ready={} second_ready={} first_diagnostics={} second_diagnostics={}",
                    report.control.total_count, confirm.control.total_count,
                    report.control.pc_read_count, confirm.control.pc_read_count,
                    report.control.pc_read_done, confirm.control.pc_read_done,
                    report.control.data_ready, confirm.control.data_ready,
                    report.diagnostics.len(), confirm.diagnostics.len()));
                return Err("Trạng thái máy đo thay đổi khi xác minh. Vui lòng thử lại sau khi máy ổn định.".into());
            }
        }
        switch_log(trace, "PLC_SNAPSHOT", "OBSERVED", format!("total={} read_count={} done={} ready={} required_seq={:?} buffers={:?} journal_raw={} diagnostics={}", report.control.total_count, report.control.pc_read_count, report.control.pc_read_done, report.control.data_ready, journal.last_total, report.buffers.iter().map(|b| (b.buffer_index, b.record.as_ref().map(|r| r.record_seq), b.raw_registers.iter().any(|w| *w != 0))).collect::<Vec<_>>(), journal.last_total.is_some_and(|seq| journal.deliveries.iter().any(|d| d.input.record_seq == seq)), report.diagnostics.len()));
        if let Some(previous) = journal.last_total {
            if report.control.total_count < previous {
                switch_log(trace, "PLC_SNAPSHOT", "COUNTER_MISMATCH", format!("expected_total_at_least={previous} actual_total={}", report.control.total_count));
                // Historical returns already hold the lock from verify_source_closed.
                // Same-endpoint reconnects must also reconcile durable history before
                // reporting that only the device's session boundary remains unknown.
                if !historical {
                    let pool = database.pool().await?;
                    let _closed_source = self.verify_source_closed(file, config, &pool, true, trace).await?;
                }
                super::connection_settings::clean_new_baseline(&report)?;
                // Only an explicit operator attestation may authorize the new session.
                // A clean snapshot is an eligibility check, never proof of physical identity.
                return Ok(true);
            }
        }
        if empty_reset {
            switch_log(trace, "JOURNAL_CHECK", "RESET_DIAGNOSTIC_PENDING", format!("last_total={:?} actual_total={} expected=confirmed_reset_baseline", journal.last_total, report.control.total_count));
            return Err("Máy đo cũ còn dấu reset nhưng counter hiện tại không ở baseline reset đã xác minh. Chưa thể tiếp tục phiên cũ.".into());
        }
        if !report.diagnostics.is_empty() {
            switch_log(trace, "PLC_SNAPSHOT", "SNAPSHOT_DIAGNOSTIC", format!("diagnostics={}", report.diagnostics.len()));
            return Err("Dữ liệu máy đo có mâu thuẫn hoặc chưa hoàn tất. Chưa thể tiếp tục phiên cũ.".into());
        }
        if historical {
            let latest_receipt = if journal.last_total.is_some_and(|seq| seq > 0)
                && !journal.deliveries.iter().any(|d| d.input.confirmed_session_id == journal.session_id
                    && Some(d.input.record_seq) == journal.last_total) {
                let pool = database.pool().await?;
                let row: Option<InboxRecord> = sqlx::query_as("SELECT i.* FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id WHERE s.source_key=$1 AND i.session_id=$2 AND i.record_seq=$3 AND i.processing_status='PROCESSED'")
                    .bind(&config.source_key).bind(&journal.session_id).bind(journal.last_total)
                    .fetch_optional(&pool).await.map_err(|error| {
                        switch_log(trace, "INBOX_CHECK", "INBOX_READ_FAILED", format!("required_seq={:?} database_error={error}", journal.last_total));
                        "Không thể đối chiếu receipt cuối trong cơ sở dữ liệu.".to_string()
                    })?;
                if row.as_ref().is_some_and(|r| r.measurement_id.is_none() || r.processed_at.is_none()
                    || r.word_order != config.word_order.as_str()) {
                    switch_log(trace, "INBOX_CHECK", "RECEIPT_MISMATCH", format!("required_seq={:?} expected=processed_receipt_with_measurement_and_word_order actual=incomplete_or_mismatch", journal.last_total));
                    return Err("Receipt cuối không đủ bằng chứng để tiếp tục phiên cũ.".into());
                }
                row
            } else { None };
            switch_log(trace, "INBOX_CHECK", "OBSERVED", format!("required_seq={:?} processed_receipt={}", journal.last_total, latest_receipt.is_some()));
            let no_valid_buffers = report.buffers.len() == super::as200::map::BUFFER_COUNT
                && report.buffers.iter().all(|b| b.record.is_none()
                    && b.raw_registers[super::as200::map::record::STATUS] & 1 == 0);
            if no_valid_buffers && journal.last_total.is_some_and(|total| total > 0)
                && config.ack_enabled {
                let checkpoint = journal.last_total.ok_or("Thiếu checkpoint của nguồn cũ; chưa thể tiếp tục.")?;
                let mut expected_binding = format!("{}|{}|{}|{}|{:?}|{}|{:?}",
                    config.mode, config.address, config.unit_id, config.source_key,
                    config.confirmed_session_id, config.modbus_address_offset, config.word_order);
                if config.ack_protocol.as_deref() == Some("as200") { expected_binding.push_str("|as200"); }
                let clean = config.ack_enabled && config.ack_protocol.as_deref() == Some("as200")
                    && checkpoint > 0 && journal.ack.accepted == checkpoint
                    && journal.ack.binding == expected_binding && journal.ack.intent.is_none()
                    && journal.deliveries.is_empty() && journal.retries.is_empty()
                    && journal.manual_retries.iter().all(|r| !matches!(r.state.as_str(), "PENDING" | "WAITING"))
                    && journal.pause.is_none() && journal.quarantine.is_empty()
                    && journal.warnings.is_empty() && journal.previous_session_warnings.is_empty()
                    && journal.session_request.is_none() && config.session_transition.is_none()
                    && report.diagnostics.is_empty() && report.new_records.is_empty()
                    && report.control.total_count == checkpoint
                    && report.control.pc_read_count == checkpoint
                    && report.control.pc_read_done == 0 && report.control.data_ready == 0;
                if !clean {
                    switch_log(trace, "BUFFER_VERIFY", "CLEAN_ACKED_GUARD_FAILED", format!(
                        "checkpoint={checkpoint} accepted={} total={} read_count={} done={} ready={} journal_deliveries={} warnings={} binding_matches={}",
                        journal.ack.accepted, report.control.total_count, report.control.pc_read_count,
                        report.control.pc_read_done, report.control.data_ready, journal.deliveries.len(),
                        journal.warnings.len(), journal.ack.binding == expected_binding));
                    return Err("Phiên cũ chưa đủ điều kiện ACK sạch để kết nối lại.".into());
                }
                let receipt = latest_receipt.as_ref().ok_or_else(|| {
                    switch_log(trace, "INBOX_CHECK", "RECEIPT_MISSING", format!("required_seq={checkpoint}"));
                    "Thiếu receipt cuối đã xử lý; chưa thể tiếp tục phiên cũ."
                })?;
                if receipt.total_count != Some(i64::from(checkpoint)) {
                    switch_log(trace, "INBOX_CHECK", "RECEIPT_MISMATCH", format!("required_seq={checkpoint} expected_total={checkpoint} actual_total={:?}", receipt.total_count));
                    return Err("Receipt cuối không khớp checkpoint; chưa thể tiếp tục phiên cũ.".into());
                }
                let pool = database.pool().await?;
                let durable_prefix: i64 = sqlx::query_scalar("SELECT count(*) FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id JOIN measurements m ON m.id=i.measurement_id AND m.source_id=i.source_id AND m.session_id=i.session_id AND m.record_seq=i.record_seq WHERE s.source_key=$1 AND i.session_id=$2 AND i.record_seq BETWEEN 1 AND $3 AND i.processing_status='PROCESSED' AND i.processed_at IS NOT NULL")
                    .bind(&config.source_key).bind(&journal.session_id).bind(checkpoint)
                    .fetch_one(&pool).await.map_err(|error| {
                        switch_log(trace, "INBOX_CHECK", "INBOX_READ_FAILED", format!("database_error={error}"));
                        "Không thể đối chiếu đầy đủ receipt phiên cũ."
                    })?;
                if durable_prefix != i64::from(checkpoint) {
                    switch_log(trace, "INBOX_CHECK", "INBOX_GAP", format!("expected_processed_prefix={checkpoint} actual_processed_prefix={durable_prefix}"));
                    return Err("Lịch sử receipt chưa đầy đủ; chưa thể tiếp tục phiên cũ.".into());
                }
                switch_log(trace, "BUFFER_VERIFY", "CLEAN_ACKED_RESUME", format!(
                    "checkpoint={checkpoint} total={} read_count={} done=0 ready=0 valid_buffers=0 processed_prefix={durable_prefix} journal_clean=true binding_matches=true snapshots_stable=true",
                    report.control.total_count, report.control.pc_read_count));
            } else {
                verify_return_snapshot_with_receipt(&journal, &report, latest_receipt.as_ref(), trace)?;
            }
            if let Some(checkpoint) = journal.last_total {
                for buffer in &report.buffers {
                    let Some(record) = &buffer.record else { continue };
                    if record.record_seq > checkpoint { continue; }
                    let pool = database.pool().await?;
                    let row: Option<InboxRecord> = sqlx::query_as("SELECT i.* FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id WHERE s.source_key=$1 AND i.session_id=$2 AND i.record_seq=$3 AND i.processing_status='PROCESSED'")
                        .bind(&config.source_key).bind(&journal.session_id).bind(record.record_seq)
                        .fetch_optional(&pool).await.map_err(|error| {
                            switch_log(trace, "INBOX_CHECK", "INBOX_READ_FAILED", format!("record_seq={} database_error={error}", record.record_seq));
                            "Không thể đối chiếu buffer cũ với Inbox."
                        })?;
                    if !row.as_ref().is_some_and(|r| r.measurement_id.is_some() && r.processed_at.is_some()
                        && r.buffer_index == buffer.buffer_index as i32 && r.word_order == config.word_order.as_str()
                        && serde_json::to_value(&buffer.raw_registers).is_ok_and(|raw| raw == r.raw_registers)) {
                        switch_log(trace, "BUFFER_VERIFY", "BUFFER_RAW_MISMATCH", format!("record_seq={} buffer_index={} expected=processed_receipt_raw actual=missing_or_different", record.record_seq, buffer.buffer_index));
                        return Err("Buffer còn hiệu lực không khớp dữ liệu đã lưu; chưa thể tiếp tục phiên cũ.".into());
                    }
                }
            }
        }
        for buffer in &report.buffers {
            if let Some(record) = &buffer.record {
                if journal.deliveries.iter().any(|d| d.input.confirmed_session_id == journal.session_id
                    && d.input.record_seq == record.record_seq && d.input.raw_registers != buffer.raw_registers) {
                    switch_log(trace, "BUFFER_VERIFY", "BUFFER_RAW_MISMATCH", format!("record_seq={} buffer_index={} expected=journal_raw actual=different", record.record_seq, buffer.buffer_index));
                    return Err("Dữ liệu máy đo không khớp lịch sử đã lưu. Vui lòng liên hệ kỹ thuật.".into());
                }
            }
        }
        Ok(false)
    }
    pub(super) async fn ensure_connection_edit_safe(
        &self,
        path: &std::path::Path,
        config: &SimulatorConfig,
    ) -> Result<Option<File>, String> {
        if self.status.read().await.enabled
            || self
                .engine
                .lock()
                .await
                .as_ref()
                .and_then(|e| e.upgrade())
                .is_some()
        {
            return Err("worker is running; no safe shutdown capability".into());
        }
        let journal_path = config.journal_file(path);
        if config.mode == "modbus"
            && !config.technical_setup_confirmed
            && !config.enabled
            && !config.ack_enabled
            && !config.pc_heartbeat_enabled
            && config.source_key.is_empty()
            && config.confirmed_session_id.is_none()
            && config.journal_path.is_none()
            && !journal_path.exists()
            && !journal_path.with_extension("lock").exists()
        {
            return Ok(None); // Endpoint-only setup has never owned measurement data.
        }
        // Never create, reset or persist the live journal from the settings flow.
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(journal_path.with_extension("lock"))
            .map_err(|e| format!("cannot verify journal lock: {e}"))?;
        lock.try_lock()
            .map_err(|e| format!("journal is owned by another worker: {e}"))?;
        let journal: Journal = serde_json::from_str(
            &std::fs::read_to_string(&journal_path).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if !matches!(journal.version, 1 | 2)
            || journal.source_key != config.source_key
            || journal.session_id != config.confirmed_session_id
            || journal.pause.is_some()
            || journal.session_request.is_some()
            || config.session_transition.is_some()
            || !journal.warnings.is_empty()
            || !journal.previous_session_warnings.is_empty()
            || journal.last_total.is_some_and(|v| v != 0)
            || journal.ack.intent.is_some()
            || journal.ack.accepted != 0
            || !journal.ack.binding.is_empty()
            || !journal.deliveries.is_empty()
            || !journal.quarantine.is_empty()
            || !journal.retries.is_empty()
            || !journal.session_archives.is_empty()
            || !journal.manual_retries.is_empty()
        {
            return Err("journal contains history or unreconciled state; endpoint binding requires technical review".into());
        }
        Ok(Some(lock))
    }
    pub async fn launch(self, app_dir: Option<PathBuf>, database: DatabaseState) {
        let gate = self.settings_gate.clone();
        let _settings_guard = gate.lock().await;
        if simulator_config::locate(app_dir.as_deref()).is_some_and(|p| p.with_extension("connection-pending").exists()) {
            *self.settings_directory.lock().await = app_dir.clone();
            *self.settings_path.lock().await = simulator_config::locate(app_dir.as_deref());
            self.settings_ready.store(true, Ordering::SeqCst);
            let mut status = self.status.write().await;
            status.state = "STOPPED".into();
            status.message = "Chuyển kết nối chưa hoàn tất. Vui lòng liên hệ kỹ thuật để đối chiếu cấu hình.".into();
            return;
        }
        self.launch_under_gate(app_dir, database).await;
    }
    pub(super) async fn launch_under_gate(self, app_dir: Option<PathBuf>, database: DatabaseState) {
        *self.database.lock().await = Some(database.clone());
        *self.settings_directory.lock().await = app_dir.clone();
        *self.settings_path.lock().await = simulator_config::locate(app_dir.as_deref());
        self.settings_ready.store(true, Ordering::SeqCst);
        if self.status.read().await.enabled || !self.tasks.lock().await.is_empty() { return; }
        if self.settings_path.lock().await.as_ref().is_some_and(|p| p.with_extension("operator-stop").exists()) {
            let mut status = self.status.write().await;
            status.state = "STOPPED".into();
            status.message = "Operator stopped collection".into();
            return;
        }
        self.stopping.store(false, Ordering::SeqCst);
        let config = match simulator_config::load(app_dir.as_deref()) {
            Ok(Some(value)) => value,
            Ok(None) => return,
            Err(error) => {
                let mut status = self.status.write().await;
                status.state = "ERROR".into();
                status.message = error;
                return;
            }
        };
        if !config.0.enabled {
            let mut status = self.status.write().await;
            status.state = "SETUP_REQUIRED".into();
            status.message =
                "Kết nối đã lưu. Cần kỹ thuật xác nhận thông số trước khi nhận dữ liệu đo.".into();
            return;
        }
        if let Err(error) = config.0.validate() {
            eprintln!("AS200 configuration rejected before connection: {error}");
            let mut status = self.status.write().await;
            status.state = "ERROR".into();
            status.message = error;
            return;
        }
        if config.0.ack_enabled && !simulator_config::ack_scope_allows(&config.0) {
            let mut status = self.status.write().await;
            status.state = "SETUP_REQUIRED".into();
            status.message = format!(
                "Bản phát hành chưa được kỹ thuật phê duyệt cho phạm vi ACK AS200 ({}).",
                simulator_config::AS200_ACK_SCOPE_ID
            );
            return;
        }
        eprintln!(
            "AS200 mode={} address={} unit={} ack_enabled={}",
            config.0.mode, config.0.address, config.0.unit_id, config.0.ack_enabled
        );
        {
            let mut status = self.status.write().await;
            status.enabled = true;
            status.address = config.0.address.clone();
            status.unit_id = config.0.unit_id;
            status.source_key = config.0.source_key.clone();
            status.session_id = None; // Only publish the session accepted by the journal.
            status.ack_status = if config.0.ack_enabled {
                "WAITING"
            } else {
                "DISABLED"
            };
            status.state = "CONNECTING".into();
            status.connection = "CONNECTING".into();
            status.message = "Đang khởi động hệ thống tiếp nhận AS200...".into();
        }
        match Engine::open(config.0, config.1, self.clone()) {
            Ok(engine) => {
                *self.engine.lock().await = Some(Arc::downgrade(&engine));
                if let Err(error) = engine.publish_journal().await {
                    engine.error(error).await;
                    self.status.write().await.enabled = false;
                    return;
                }
                let mut tasks = self.tasks.lock().await;
                tasks.push(tauri::async_runtime::spawn(engine.clone().heartbeat_loop()));
                tasks.push(tauri::async_runtime::spawn(engine.clone().reader_loop(database.clone())));
                tasks.push(tauri::async_runtime::spawn(engine.sink_loop(database)));
            }
            Err(error) => {
                let mut status = self.status.write().await;
                status.enabled = false;
                status.state = "ERROR".into();
                status.message = error;
            }
        }
    }
}
#[tauri::command]
pub async fn retry_as200_inbox(
    inbox_id: i64,
    state: tauri::State<'_, WorkerState>,
) -> Result<(), String> {
    if inbox_id <= 0 || !state.status.read().await.enabled {
        return Err("Mã tiếp nhận không hợp lệ hoặc worker chưa bật".into());
    }
    let engine = state
        .engine
        .lock()
        .await
        .as_ref()
        .and_then(|e| e.upgrade())
        .ok_or("Worker journal is not available")?;
    engine.queue_retry(inbox_id).await?;
    engine.publish_journal().await
}
#[tauri::command]
pub async fn get_as200_retry_diagnostics(
    state: tauri::State<'_, WorkerState>,
) -> Result<Vec<String>, String> {
    Ok(state.status.read().await.retry_diagnostics.clone())
}
#[tauri::command]
pub async fn get_as200_worker_status(
    state: tauri::State<'_, WorkerState>,
) -> Result<WorkerStatus, String> {
    state.fresh_health().await;
    Ok(state.status.read().await.clone())
}
#[tauri::command]
pub async fn reconnect_as200_plc(state: tauri::State<'_, WorkerState>) -> Result<(), String> {
    let status = state.status.read().await;
    if !status.enabled || status.connection != "DISCONNECTED"
        || status.pause_reason.is_some() || matches!(status.state.as_str(), "STOPPING" | "STOPPED")
    {
        return Err("Worker chưa ở trạng thái mất kết nối có thể thử lại; không thay đổi trạng thái thu.".into());
    }
    drop(status);
    if state.stopping.load(Ordering::SeqCst)
        || state.engine.lock().await.as_ref().and_then(|engine| engine.upgrade()).is_none()
    {
        return Err("Worker chưa sẵn sàng để kết nối lại.".into());
    }
    let (sender, receiver) = oneshot::channel();
    {
        let mut waiters = state.reconnect_waiters.lock().await;
        if state.stopping.load(Ordering::SeqCst) {
            return Err("Worker đang dừng; không tự kết nối lại.".into());
        }
        waiters.push(sender);
    }
    state.reconnect_now.notify_one();
    tokio::time::timeout(Duration::from_secs(30), receiver)
        .await
        .map_err(|_| "Chưa đọc lại được PLC trong thời gian chờ; Worker vẫn tự thử lại.".to_string())?
        .map_err(|_| "Worker đã dừng trước khi hoàn tất kiểm tra PLC.".to_string())?
}
#[tauri::command]
pub async fn acknowledge_as200_buffer_gap(
    state: tauri::State<'_, WorkerState>,
) -> Result<(), String> {
    if !matches!(
        state.status.read().await.pause_reason.as_deref(),
        Some("OVERFLOW" | "QUEUE_FULL")
    ) {
        return Err("Chỉ được tiếp tục sau cảnh báo tràn buffer đã kiểm tra. Reset/xung đột phiên cần cấu hình lại rõ ràng.".into());
    }
    state.ack_gap.store(true, Ordering::SeqCst);
    Ok(())
}
#[cfg(test)]
mod tests;
