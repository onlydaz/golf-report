use super::as200;
use super::{
    as200::{
        As200Error,
        ack::{PublishedAckIntent, PublishedAckPhase},
        decode,
        reader::{As200Reader, PublishedRecordSnapshot},
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
mod ack;
mod capture;
mod delivery;
mod journal;
mod recovery;
mod retry;
pub use journal::ManualRetryRequest;
use journal::{Delivery, Journal, QuarantinedRecord};

pub(super) fn switch_log(trace: &str, step: &str, reason: &str, detail: impl std::fmt::Display) {
    eprintln!("PLC_SWITCH trace={trace} step={step} reason={reason} {detail}");
}

fn processed_receipt_matches(row: &InboxRecord, input: &InboxInput) -> bool {
    row.session_id == input.confirmed_session_id
        && row.record_seq == input.record_seq
        && row.total_count == input.total_count
        && row.ingest_key == input.ingest_key
        && row.word_order == input.word_order.as_str()
        && row.buffer_index == input.buffer_index
        && row.received_at.timestamp_micros() == input.received_at.timestamp_micros()
        && serde_json::to_value(&input.raw_registers).is_ok_and(|raw| raw == row.raw_registers)
        && row.processing_status == "PROCESSED"
        && row.measurement_id.is_some()
        && row.processed_at.is_some()
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
            ack_status: "DISABLED",
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
    reconnect_now: Arc<Notify>,
    reconnect_waiters: Arc<Mutex<Vec<oneshot::Sender<Result<(), String>>>>>,
}
impl WorkerState {
    // Caller holds settings_gate through the manual operation, preventing a
    // concurrent collection start. Do not infer safety from enabled alone.
    pub(super) async fn ensure_register_writes_stopped(&self) -> Result<(), String> {
        if !self.settings_ready.load(Ordering::SeqCst)
            || self.runtime_error.lock().await.is_some()
            || self.status.read().await.enabled
            || self.status.read().await.state == "STOPPING"
            || !self.tasks.lock().await.is_empty()
            || self
                .engine
                .lock()
                .await
                .as_ref()
                .and_then(|e| e.upgrade())
                .is_some()
        {
            return Err("Chưa xác minh bộ thu/ACK đã dừng hoàn toàn. Hãy dừng thu và xử lý lỗi runtime trước khi ghi thanh ghi.".into());
        }
        Ok(())
    }

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
        let path = self
            .settings_path
            .lock()
            .await
            .clone()
            .ok_or("Chưa có cấu hình kết nối.")?;
        let engine = self
            .engine
            .lock()
            .await
            .as_ref()
            .and_then(|engine| engine.upgrade());
        let marker = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path.with_extension("operator-stop"))
            .map_err(|_| "Không thể ghi nhận yêu cầu dừng.")?;
        marker
            .sync_all()
            .map_err(|_| "Không thể lưu yêu cầu dừng.")?;
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
        let flush_result =
            if let (Some(engine), Some(database)) = (engine, self.database.lock().await.clone()) {
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
        health == Some("RUNNING") && status.plc_health == "RUNNING"
    }
}
pub(super) struct Engine {
    config: SimulatorConfig,
    path: PathBuf,
    journal: Mutex<Journal>,
    state: WorkerState,
    _lock: File,
    published_gate: Mutex<()>,
}
impl Engine {
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

    async fn read_snapshot(&self) -> Result<PublishedRecordSnapshot, String> {
        match As200Reader::new(self.config.reader())
            .poll_published_record()
            .await
        {
            Ok(snapshot) => {
                let health = self
                    .state
                    .heartbeat
                    .lock()
                    .unwrap()
                    .get_or_insert_with(|| {
                        super::as200::heartbeat::HeartbeatMonitor::new(
                            self.config.heartbeat_timeout(),
                            std::time::Instant::now(),
                        )
                    })
                    .observe(snapshot.control.heartbeat, std::time::Instant::now());
                let mut status = self.state.status.write().await;
                status.connection = "CONNECTED".into();
                status.plc_health = health.into();
                Ok(snapshot)
            }
            Err(error) => {
                if matches!(error, As200Error::Transport(_) | As200Error::Timeout) {
                    let mut status = self.state.status.write().await;
                    status.connection = "DISCONNECTED".into();
                    status.plc_health = "DISCONNECTED".into();
                }
                Err(error.to_string())
            }
        }
    }

    pub(super) async fn poll_and_ack_published(&self, pool: &PgPool) -> Result<(), String> {
        let _gate = self.published_gate.lock().await;
        let result: Result<(), String> = async {
            self.flush(pool).await?;
            self.verify_published_history(pool).await?;
            if let Some(pause) = &self.journal.lock().await.pause {
                return Err(format!("Journal v4 đang dừng bảo vệ dữ liệu: {pause}."));
            }
            let snapshot = self.read_snapshot().await?;
            self.published_ack_step(snapshot, pool).await?;
            let mut journal = self.journal.lock().await;
            let progress = journal
                .published_ack
                .as_ref()
                .ok_or("Thiếu checkpoint v4.")?;
            if journal.pause.is_none()
                && !journal.pending_ack()
                && journal.last_total == Some(progress.accepted)
            {
                // This iteration proved the DB prefix and matching idle PLC
                // checkpoint. Only resolved transient diagnostics may disappear.
                let before = journal.warnings.len();
                journal
                    .warnings
                    .retain(|w| !w.starts_with("PUBLISHED_ACK:"));
                if before != journal.warnings.len() {
                    self.persist(&journal)?;
                }
                let mut status = self.state.status.write().await;
                status.warnings = journal.warnings.clone();
                if status
                    .latest
                    .as_ref()
                    .is_none_or(|r| r.save_state == "PROCESSED")
                {
                    status.state = if status.latest.is_some() {
                        "PROCESSED"
                    } else {
                        "WAITING"
                    }
                    .into();
                    status.message = "Đã kết nối máy đo, đang chờ kết quả mới.".into();
                }
            }
            Ok(())
        }
        .await;
        if let Err(error) = &result {
            // Uncertain does not mean failed. Keep observing on a later explicit
            // iteration; do not replay an uncertain Count or Done.
            let mut journal = self.journal.lock().await;
            Self::warn(&mut journal, format!("PUBLISHED_ACK: {error}"));
            if let Err(persist_error) = self.persist(&journal) {
                journal.pause = Some("JOURNAL_WRITE_FAILED".into());
                drop(journal);
                self.error(persist_error.clone()).await;
                return Err(format!("{error}; {persist_error}"));
            }
            drop(journal);
            self.state.status.write().await.ack_status = "RECONCILIATION_REQUIRED";
            self.error(error.clone()).await;
        }
        result
    }

    async fn collection_loop(self: Arc<Self>, database: DatabaseState) {
        loop {
            if self.state.stopping.load(Ordering::SeqCst) {
                break;
            }
            let waiters = std::mem::take(&mut *self.state.reconnect_waiters.lock().await);
            let result = tokio::time::timeout(Duration::from_secs(8), async {
                let pool = database.pool().await?;
                self.poll_and_ack_published(&pool).await
            }).await.unwrap_or_else(|_| Err("Hết thời gian đọc/lưu; giữ receipt và ACK intent để phục hồi, không phát lại lệnh chưa rõ kết quả.".into()));
            if let Err(error) = &result {
                self.error(error.clone()).await;
            }
            let reconnect = if result.is_ok()
                && self.state.status.read().await.ack_status == "RECONCILIATION_REQUIRED"
            {
                Err("PLC còn ACK cần đối chiếu; chưa thể xác nhận tiếp tục thu an toàn.".into())
            } else {
                result
            };
            for waiter in waiters {
                let _ = waiter.send(reconnect.clone());
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(self.config.poll_interval_ms)) => {},
                _ = self.state.reconnect_now.notified() => {},
            }
        }
    }
}
impl WorkerState {
    pub async fn launch(self, app_dir: Option<PathBuf>, database: DatabaseState) {
        let gate = self.settings_gate.clone();
        let _settings_guard = gate.lock().await;
        if simulator_config::locate(app_dir.as_deref())
            .is_some_and(|p| p.with_extension("connection-pending").exists())
        {
            *self.settings_directory.lock().await = app_dir.clone();
            *self.settings_path.lock().await = simulator_config::locate(app_dir.as_deref());
            self.settings_ready.store(true, Ordering::SeqCst);
            let mut status = self.status.write().await;
            status.state = "STOPPED".into();
            status.message =
                "Chuyển kết nối chưa hoàn tất. Vui lòng liên hệ kỹ thuật để đối chiếu cấu hình."
                    .into();
            return;
        }
        self.launch_under_gate(app_dir, database).await;
    }
    pub(super) async fn launch_under_gate(self, app_dir: Option<PathBuf>, database: DatabaseState) {
        *self.database.lock().await = Some(database.clone());
        *self.settings_directory.lock().await = app_dir.clone();
        *self.settings_path.lock().await = simulator_config::locate(app_dir.as_deref());
        self.settings_ready.store(true, Ordering::SeqCst);
        if self.status.read().await.enabled || !self.tasks.lock().await.is_empty() {
            return;
        }
        if self
            .settings_path
            .lock()
            .await
            .as_ref()
            .is_some_and(|p| p.with_extension("operator-stop").exists())
        {
            let mut status = self.status.write().await;
            status.state = "STOPPED".into();
            status.message = "Operator stopped collection".into();
            return;
        }
        self.stopping.store(false, Ordering::SeqCst);
        *self.heartbeat.lock().unwrap() = None; // Never reuse heartbeat proof from another PLC.
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
            status.plc_health = "OBSERVING".into();
            status.message = "Đang khởi động hệ thống tiếp nhận AS200...".into();
        }
        match Engine::open(config.0, config.1, self.clone()) {
            Ok(engine) => {
                let result = async {
                    let pool = database.pool().await?;
                    engine.verify_open(&pool).await
                }
                .await;
                if let Err(error) = result {
                    engine.error(error).await;
                    self.status.write().await.enabled = false;
                    return;
                }
                *self.engine.lock().await = Some(Arc::downgrade(&engine));
                self.tasks.lock().await.push(tauri::async_runtime::spawn(
                    engine.collection_loop(database),
                ));
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
    if !status.enabled
        || status.connection != "DISCONNECTED"
        || status.pause_reason.is_some()
        || matches!(status.state.as_str(), "STOPPING" | "STOPPED")
    {
        return Err(
            "Worker chưa ở trạng thái mất kết nối có thể thử lại; không thay đổi trạng thái thu."
                .into(),
        );
    }
    drop(status);
    if state.stopping.load(Ordering::SeqCst)
        || state
            .engine
            .lock()
            .await
            .as_ref()
            .and_then(|engine| engine.upgrade())
            .is_none()
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
        .map_err(|_| {
            "Chưa đọc lại được PLC trong thời gian chờ; Worker vẫn tự thử lại.".to_string()
        })?
        .map_err(|_| "Worker đã dừng trước khi hoàn tất kiểm tra PLC.".to_string())?
}

#[tauri::command]
pub async fn acknowledge_as200_buffer_gap(
    _state: tauri::State<'_, WorkerState>,
) -> Result<(), String> {
    Err("Published Record không cho phép bỏ qua sequence gap; cần kỹ thuật đối chiếu dữ liệu và ACK intent.".into())
}

#[cfg(test)]
mod tests;
