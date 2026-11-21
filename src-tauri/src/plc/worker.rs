use super::as200;
use super::{
    as200::{As200Error, decode, reader::PublishedRecordSnapshot},
    simulator_config::{self, SimulatorConfig},
};
use crate::{
    database::DatabaseState,
    models::InboxRecord,
    repositories::{InboxRepository, SourceRepository},
    services::InboxService,
};
use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
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
mod acquisition;
mod connection_log;
mod legacy;
mod store;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestRecord {
    pub quality_error: Option<String>,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionEvent {
    pub at: DateTime<Utc>,
    pub kind: String,
    pub endpoint: String,
    pub message: String,
}

/// In-memory observations only; never restored from a journal or inferred from UI polling.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadTelemetry {
    pub date: NaiveDate,
    pub observed_since: DateTime<Utc>,
    pub read_errors: u64,
    pub disconnects: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawCaptureStatus {
    pub inbox_id: i64,
    pub received_at: DateTime<Utc>,
    pub total_count: i32,
    pub ack_status: &'static str,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerStatus {
    pub raw_capture: Option<RawCaptureStatus>,
    pub read_telemetry: Option<ReadTelemetry>,
    pub control_snapshot: Option<as200::model::PlcControl>,
    pub connected_at: Option<DateTime<Utc>>,
    pub connection_ended_at: Option<DateTime<Utc>>,
    pub last_read_at: Option<DateTime<Utc>>,
    pub last_read_ms: Option<u64>,
    pub last_error: Option<String>,
    pub connection_events: Vec<ConnectionEvent>,
    pub connection_log_error: Option<String>,
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
    pub latest: Option<LatestRecord>,
    pub last_saved_id: Option<i64>,
}
impl Default for WorkerStatus {
    fn default() -> Self {
        Self {
            raw_capture: None,
            read_telemetry: None,
            control_snapshot: None,
            connected_at: None,
            connection_ended_at: None,
            last_read_at: None,
            last_read_ms: None,
            last_error: None,
            connection_events: Vec::new(),
            connection_log_error: None,
            blocked_inbox_ids: Vec::new(),
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
            latest: None,
            last_saved_id: None,
        }
    }
}
impl WorkerStatus {
    pub(super) fn confirm_read_only_probe(&mut self) {
        if !self.enabled
            && self.state == "ERROR"
            && self.pause_reason.is_none()
            && !matches!(self.ack_status, "FAILED" | "RECONCILIATION_REQUIRED")
            && self.blocked_inbox_count == 0
            && ["Lỗi Modbus TCP AS200:", "Hết thời gian kết nối/đọc AS200"]
                .iter()
                .any(|error| self.message.contains(error))
        {
            self.state = "STOPPED".into();
            self.last_error = None;
            self.message = "Đã kiểm tra kết nối PLC; bộ thu chưa được khởi động.".into();
        }
    }
    fn telemetry_day(now: DateTime<Utc>) -> NaiveDate {
        now.with_timezone(&FixedOffset::east_opt(7 * 3600).unwrap())
            .date_naive()
    }

    fn rotate_telemetry(&mut self, now: DateTime<Utc>) {
        if let Some(telemetry) = &mut self.read_telemetry {
            let date = Self::telemetry_day(now);
            if telemetry.date != date {
                let midnight = date
                    .and_hms_opt(0, 0, 0)
                    .unwrap()
                    .and_local_timezone(FixedOffset::east_opt(7 * 3600).unwrap())
                    .single()
                    .unwrap()
                    .with_timezone(&Utc);
                *telemetry = ReadTelemetry {
                    date,
                    observed_since: midnight,
                    read_errors: 0,
                    disconnects: 0,
                };
            }
        }
    }

    fn observe_read(&mut self, now: DateTime<Utc>, error: Option<&As200Error>) {
        self.rotate_telemetry(now);
        let telemetry = self.read_telemetry.get_or_insert_with(|| ReadTelemetry {
            date: Self::telemetry_day(now),
            observed_since: now,
            read_errors: 0,
            disconnects: 0,
        });
        // An unstable snapshot is an expected concurrent PLC update, not a failed read.
        if error.is_some_and(|e| !matches!(e, As200Error::Unstable)) {
            telemetry.read_errors = telemetry.read_errors.saturating_add(1);
        }
        if error.is_some_and(|e| matches!(e, As200Error::Transport(_) | As200Error::Timeout))
            && self.connection == "CONNECTED"
        {
            telemetry.disconnects = telemetry.disconnects.saturating_add(1);
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
    startup_config: Arc<Mutex<Option<SimulatorConfig>>>,
    connection_log: Arc<Mutex<connection_log::ConnectionLog>>,
    diagnostic_logs: Arc<std::sync::Mutex<super::diagnostics::ChangedLogs>>,
    pub(super) settings_ready: Arc<AtomicBool>,
    pub(super) runtime_error: Arc<Mutex<Option<String>>>,
    heartbeat: Arc<std::sync::Mutex<Option<super::as200::heartbeat::HeartbeatMonitor>>>,
    engine: Arc<Mutex<Option<std::sync::Weak<Engine>>>>,
    pub status: Arc<RwLock<WorkerStatus>>,
    reconnect_now: Arc<Notify>,
    reconnect_waiters: Arc<Mutex<Vec<oneshot::Sender<Result<(), String>>>>>,
}
impl WorkerState {
    // These caches affect stderr only. Fail open on a poisoned cache so a
    // diagnostic failure never hides evidence or changes acquisition behavior.
    fn log_changed(&self, event: &'static str, value: String) -> bool {
        self.diagnostic_logs
            .lock()
            .map(|mut logs| logs.should_emit(event, value))
            .unwrap_or(true)
    }

    fn forget_log(&self, event: &'static str) -> bool {
        self.diagnostic_logs
            .lock()
            .map(|mut logs| logs.forget(event))
            .unwrap_or(false)
    }

    fn reset_diagnostic_logs(&self) {
        if let Ok(mut logs) = self.diagnostic_logs.lock() {
            logs.clear();
        }
    }

    pub(super) async fn tasks_are_stopped(&self) -> bool {
        self.tasks.lock().await.is_empty()
    }
    pub(super) async fn engine_for_diagnostic_read(&self) -> Option<Arc<Engine>> {
        self.engine
            .lock()
            .await
            .as_ref()
            .and_then(|engine| engine.upgrade())
    }

    pub(super) async fn require_startup_environment(
        &self,
        config: &SimulatorConfig,
    ) -> Result<(), String> {
        let startup = self.startup_config.lock().await;
        if let Some(initial) = startup.as_ref() {
            if !initial.same_environment(config) {
                return Err(format!(
                    "Cấu hình đã thay đổi. Worker đã nạp ack_enabled={}, read_only={}; TOML hiện tại có ack_enabled={}, read_only={}. Dừng thu dữ liệu, thoát và khởi động lại Golf Report để áp dụng; không tự bật ACK khi ứng dụng đang chạy.",
                    initial.ack_enabled, initial.read_only, config.ack_enabled, config.read_only
                ));
            }
        }
        if startup.is_none() {
            return Err("Cấu hình môi trường đã thay đổi. Dừng thu dữ liệu, thoát và khởi động lại Golf Report để áp dụng; không chuyển nguồn khi ứng dụng đang chạy.".into());
        }
        Ok(())
    }


    async fn error_status(&self, error: String) {
        let mut status = self.status.write().await;
        status.enabled = false;
        status.connection = "DISABLED".into();
        status.state = "ERROR".into();
        status.message = error;
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
        let was_active = self.status.read().await.enabled;
        let path = self
            .settings_path
            .lock()
            .await
            .clone()
            .ok_or("Chưa có cấu hình kết nối.")?;
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
        self.reconnect_now.notify_one();
        {
            let mut status = self.status.write().await;
            status.state = "STOPPING".into();
            if status.connected_at.is_some() && status.connection_ended_at.is_none() {
                status.connection_ended_at = Some(Utc::now());
            }
        }
        // Keep handles in shared state: cancelling the command must not detach unfinished tasks.
        let mut tasks = self.tasks.lock().await;
        let mut task_error = None;
        while !tasks.is_empty() {
            if (&mut tasks[0]).await.is_err() {
                task_error = Some("Worker gặp lỗi khi dừng.".to_string());
            }
            tasks.remove(0);
        }
        for waiter in std::mem::take(&mut *self.reconnect_waiters.lock().await) {
            let _ = waiter.send(Err("Đã dừng thu dữ liệu; không tự kết nối lại.".into()));
        }
        let stop_result = task_error.map_or(Ok(()), Err);
        *self.engine.lock().await = None;
        let mut status = self.status.write().await;
        status.enabled = false;
        status.connection = "DISABLED".into();
        status.state = "STOPPED".into();
        status.message = if stop_result.is_ok() {
            "Đã dừng thu dữ liệu.".into()
        } else {
            "Đã dừng thu; raw đã commit và ACK đang chờ vẫn nằm trong PostgreSQL.".into()
        };
        let endpoint = status.address.clone();
        drop(status);
        if was_active {
            self.record_connection("MANUAL_STOP", &endpoint).await;
        }
        stop_result
    }
    async fn fresh_health(&self) -> bool {
        let mut status = self.status.write().await;
        // This configuration choice is diagnostic only, not an operator warning.
        // Status polling also removes old entries while the Worker is idle.
        status
            .warnings
            .retain(|warning| !warning.starts_with("ACK_HEARTBEAT_BYPASS:"));
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

    // Used only after a successful, matching Modbus snapshot before an ACK write.
    // Monitoring remains independent: bypass never changes STALE to RUNNING.
    async fn heartbeat_allows_ack(&self, required: bool) -> bool {
        const WARNING: &str = "ACK_HEARTBEAT_BYPASS: require_plc_heartbeat_for_ack=false; cho phép ACK khi heartbeat OBSERVING/STALE, vẫn kiểm tra các điều kiện an toàn khác.";
        let running = self.fresh_health().await;
        let status = self.status.read().await;
        let observed_health = self
            .heartbeat
            .lock()
            .unwrap()
            .as_ref()
            .map(|m| m.health(std::time::Instant::now()));
        let bypass = !required
            && status.connection == "CONNECTED"
            && matches!(observed_health, Some("OBSERVING" | "STALE"))
            && observed_health == Some(status.plc_health.as_str());
        let allowed = status.connection == "CONNECTED" && (running || bypass);
        let policy = format!("required={required} connection={} health={} observed={observed_health:?} running={running} bypass={bypass} allowed={allowed}", status.connection, status.plc_health);
        if self.log_changed("WORKER_HEARTBEAT_POLICY", policy) {
            if bypass {
                eprintln!("PLC_WORKER {WARNING} health={}", status.plc_health);
            } else {
                eprintln!("PLC_WORKER stage=ACK_HEARTBEAT_POLICY required={required} health={} connection={} allowed={allowed}", status.plc_health, status.connection);
            }
        }
        allowed
    }
}
pub(super) struct Engine {
    config: SimulatorConfig,
    source_id: i64,
    projection_due: Mutex<std::time::Instant>,
    projection_cursors: Mutex<(i64, i64)>,
    state: WorkerState,
    _lock: File,
    published_gate: Mutex<()>,
}
impl Engine {
    pub(super) fn connection_config(&self) -> SimulatorConfig {
        self.config.clone()
    }
    pub(super) fn try_diagnostic_read_lock(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, ()>, String> {
        self.published_gate.try_lock().map_err(|_| {
            "Worker đang xử lý một chu kỳ PLC; chưa đọc chẩn đoán. Vui lòng thử lại.".into()
        })
    }

    async fn error(&self, message: String) {
        let mut status = self.state.status.write().await;
        let mut comparison = status.control_snapshot.clone();
        if let Some(control) = comparison.as_mut() {
            control.heartbeat = 0; // Heartbeat increments alone are not a new ACK error.
        }
        let error_key = format!("endpoint={} connection={} health={} pause={:?} ack={} control={comparison:?} reason={message}", self.config.address, status.connection, status.plc_health, status.pause_reason, status.ack_status);
        if self.state.log_changed("WORKER_ERROR", error_key) {
            eprintln!(
                "PLC_WORKER state=ERROR endpoint={} pause={:?} ack={} control={:?} reason={message}",
                self.config.address,
                status.pause_reason,
                status.ack_status,
                status.control_snapshot
            );
        }
        status.state = if status.pause_reason.is_some() {
            "PAUSED"
        } else {
            "ERROR"
        }
        .into();
        status.last_error = Some(message.clone());
        status.message = message;
    }

    async fn read_snapshot(&self) -> Result<PublishedRecordSnapshot, String> {
        let started = std::time::Instant::now();
        match self.config.plc_reader().poll_published_record().await {
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
                if !self.state.stopping.load(Ordering::SeqCst) {
                    // A complete, stable raw read succeeded even if measurement
                    // decoding is unavailable. Transport/control errors use Err below.
                    status.observe_read(Utc::now(), None);
                }
                let connection_event = if status.connection != "CONNECTED" {
                    let kind = if status.connection == "DISCONNECTED" {
                        "RECONNECTED"
                    } else {
                        "CONNECTED"
                    };
                    status.connected_at = Some(Utc::now());
                    status.connection_ended_at = None;
                    Some(kind)
                } else {
                    None
                };
                status.control_snapshot = Some(snapshot.control.clone());
                status.last_read_at = Some(Utc::now());
                status.last_read_ms = Some(started.elapsed().as_millis() as u64);
                status.connection = "CONNECTED".into();
                status.plc_health = health.into();
                drop(status);
                if !self.state.stopping.load(Ordering::SeqCst) {
                    if let Some(kind) = connection_event {
                        eprintln!("PLC_WORKER connection={kind} endpoint={} unit_id={} control={:?}", self.config.address, self.config.unit_id, snapshot.control);
                        self.state
                            .record_connection(kind, &self.config.address)
                            .await;
                    }
                }
                Ok(snapshot)
            }
            Err(error) => {
                let mut status = self.state.status.write().await;
                let previous = status.connection.clone();
                if !self.state.stopping.load(Ordering::SeqCst) {
                    status.observe_read(Utc::now(), Some(&error));
                }
                if matches!(error, As200Error::Transport(_) | As200Error::Timeout) {
                    if previous == "CONNECTED" && status.connection_ended_at.is_none() {
                        status.connection_ended_at = Some(Utc::now());
                    }
                    status.connection = "DISCONNECTED".into();
                    status.plc_health = "DISCONNECTED".into();
                } else if matches!(error, As200Error::InvalidData(_) | As200Error::Protocol(_)) {
                    status.plc_health = "INVALID_DATA".into();
                }
                drop(status);
                if !self.state.stopping.load(Ordering::SeqCst)
                    && (matches!(error, As200Error::Transport(_) | As200Error::Timeout)
                        || previous != "CONNECTED")
                {
                    let kind = if previous == "CONNECTED" {
                        "DISCONNECTED"
                    } else {
                        "FAILED"
                    };
                    self.state
                        .record_connection(kind, &self.config.address)
                        .await;
                }
                Err(error.to_string())
            }
        }
    }

    async fn collection_loop(self: Arc<Self>, database: DatabaseState) {
        loop {
            if self.state.stopping.load(Ordering::SeqCst) {
                break;
            }
            let waiters = std::mem::take(&mut *self.state.reconnect_waiters.lock().await);
            if !self.config.auto_reconnect
                && waiters.is_empty()
                && self.state.status.read().await.connection == "DISCONNECTED"
            {
                tokio::select! {
                    _ = self.state.reconnect_now.notified() => {},
                    _ = tokio::time::sleep(Duration::from_millis(200)) => {},
                }
                continue;
            }
            // Up to six ready snapshots (9 FC03 each), plus prepared sockets
            // and two writes. Projection has an independent timeout below.
            let result = tokio::time::timeout(Duration::from_millis(self.config.timeout_ms.saturating_mul(64).saturating_add(8000)), async {
                let pool = database.pool().await?;
                self.poll_and_ack_published(&pool).await
            }).await.unwrap_or_else(|_| Err("Hết thời gian đọc/lưu; giữ receipt và ACK intent để phục hồi, không phát lại lệnh chưa rõ kết quả.".into()));
            if !self.state.stopping.load(Ordering::SeqCst) {
                let projection = tokio::time::timeout(
                    Duration::from_millis(
                        self.config
                            .timeout_ms
                            .saturating_mul(12)
                            .saturating_add(8000),
                    ),
                    async {
                        let pool = database.pool().await?;
                        self.project_pending(&pool).await
                    },
                )
                .await
                .unwrap_or_else(|_| Err("Hết thời gian projection; retry cùng Inbox ID.".into()));
                if let Err(error) = projection {
                    let mut status = self.state.status.write().await;
                    status.warnings.retain(|w| !w.starts_with("MEASUREMENT:"));
                    status.warnings.push(format!("MEASUREMENT: {error}"));
                }
            }
            if let Err(error) = &result {
                if let Ok(pool) = database.pool().await {
                    if let Ok(Some(p)) = store::progress(&pool, self.source_id).await {
                        let mut status = self.state.status.write().await;
                        if p.ack_attempted || p.blocked_reason.is_some() {
                            status.ack_status = "RECONCILIATION_REQUIRED";
                        } else if !matches!(
                            status.ack_status,
                            "DISABLED" | "PENDING" | "RECONCILIATION_REQUIRED"
                        ) {
                            status.ack_status = "FAILED";
                        }
                    }
                }
                self.error(error.clone()).await;
            } else if self.state.forget_log("WORKER_ERROR") {
                let status = self.state.status.read().await;
                eprintln!("PLC_WORKER stage=POLL_RECOVERED endpoint={} pause={:?} ack={} control={:?}", self.config.address, status.pause_reason, status.ack_status, status.control_snapshot);
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
    /// Startup prepares settings only, regardless of the persisted enabled flag.
    /// Only the explicit Connect command may open an Engine/socket.
    pub async fn initialize(self, app_dir: Option<PathBuf>, database: DatabaseState) {
        let gate = self.settings_gate.clone();
        let _settings_guard = gate.lock().await;
        *self.database.lock().await = Some(database);
        *self.settings_directory.lock().await = app_dir.clone();
        if let Some(directory) = app_dir.as_deref() {
            self.load_connection_log(directory).await;
        }
        *self.settings_path.lock().await = simulator_config::locate(app_dir.as_deref());
        self.settings_ready.store(true, Ordering::SeqCst);
        match simulator_config::load(app_dir.as_deref()) {
            Ok(Some((config, _))) => {
                *self.startup_config.lock().await = Some(config.clone());
                let mut status = self.status.write().await;
                status.address = config.address;
                status.unit_id = config.unit_id;
                status.source_key = config.source_key;
                status.state = "STOPPED".into();
                status.message =
                    "Chưa kết nối PLC. Nhấn Kiểm tra kết nối hoặc Kết nối khi cần.".into();
            }
            Ok(None) => {}
            Err(error) => self.error_status(error).await,
        }
    }
    pub(super) async fn launch_under_gate(self, app_dir: Option<PathBuf>, database: DatabaseState) {
        *self.database.lock().await = Some(database.clone());
        *self.settings_directory.lock().await = app_dir.clone();
        if let Some(directory) = app_dir.as_deref() {
            self.load_connection_log(directory).await;
        }
        *self.settings_path.lock().await = simulator_config::locate(app_dir.as_deref());
        self.settings_ready.store(true, Ordering::SeqCst);
        if self.status.read().await.enabled || !self.tasks.lock().await.is_empty() {
            return;
        }
        self.stopping.store(false, Ordering::SeqCst);
        *self.heartbeat.lock().unwrap() = None; // Restart requires fresh heartbeat evidence.
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
        self.startup_config
            .lock()
            .await
            .get_or_insert_with(|| config.0.clone());
        if let Err(error) = self.require_startup_environment(&config.0).await {
            self.error_status(error).await;
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
            status.message =
                "Đã dừng thu dữ liệu. Nhấn Kết nối để tiếp tục với cấu hình đã nạp.".into();
            return;
        }
        if let Err(error) = super::connection_settings::validate_config(&config.0) {
            self.error_status(error).await;
            return;
        }
        if !config.0.enabled {
            let mut status = self.status.write().await;
            status.state = "SETUP_REQUIRED".into();
            status.message =
                "Bộ thu đang tắt. Nhấn Kết nối để kiểm tra và bắt đầu thu khi đủ điều kiện an toàn.".into();
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
            "AS200 mode={} address={} unit={} ack_enabled={} read_only={} pid={} ack_config_reason={}",
            config.0.mode, config.0.address, config.0.unit_id, config.0.ack_enabled,
            config.0.read_only, std::process::id(),
            config.0.ack_config_block_reason().unwrap_or("Đã cấp quyền trong cấu hình; chờ receipt PostgreSQL, publication identity, snapshot và heartbeat")
        );
        {
            *self.heartbeat.lock().unwrap() = None;
            let mut status = self.status.write().await;
            status.control_snapshot = None;
            status.raw_capture = None;
            status.connected_at = None;
            status.connection_ended_at = None;
            status.last_read_at = None;
            status.last_read_ms = None;
            status.enabled = true;
            status.address = config.0.address.clone();
            status.unit_id = config.0.unit_id;
            status.source_key = config.0.source_key.clone();
            status.session_id = None; // Published after verifying PostgreSQL binding.
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
        let opened = async {
            let pool = database.pool().await?;
            Engine::open(config.0, config.1, self.clone(), &pool).await
        }
        .await;
        match opened {
            Ok(engine) => {
                *self.engine.lock().await = Some(Arc::downgrade(&engine));
                self.tasks.lock().await.push(tauri::async_runtime::spawn(
                    engine.collection_loop(database),
                ));
            }
            Err(error) => {
                self.error_status(error).await;
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
        .ok_or("Worker chưa sẵn sàng")?;
    let _gate = engine.published_gate.lock().await;
    let database = state
        .database
        .lock()
        .await
        .clone()
        .ok_or("Database chưa sẵn sàng")?;
    let pool = database.pool().await?;
    let row = InboxRepository::find(&pool, inbox_id).await?;
    if row.source_id != engine.source_id {
        return Err("Inbox không thuộc nguồn đang chạy".into());
    }
    InboxService::process(&pool, inbox_id).await?;
    engine.refresh_display(&pool).await
}

#[tauri::command]
pub async fn get_as200_worker_status(
    state: tauri::State<'_, WorkerState>,
) -> Result<WorkerStatus, String> {
    state.fresh_health().await;
    let mut status = state.status.write().await;
    status.rotate_telemetry(Utc::now());
    Ok(status.clone())
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

#[cfg(test)]
mod tests;
