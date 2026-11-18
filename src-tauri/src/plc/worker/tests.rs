use super::*;
use crate::database::test_support::{receipt, registers};
use store::Progress;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn worker_status_keeps_receipt_metadata_without_legacy_or_raw_payload_fields() {
    let status = WorkerStatus {
        raw_capture: Some(RawCaptureStatus {
            inbox_id: 42,
            received_at: Utc::now(),
            total_count: 7,
            ack_status: "ACKNOWLEDGED",
        }),
        ..Default::default()
    };
    let json = serde_json::to_value(status).unwrap();
    for removed in [
        "publishedRecordSeq",
        "manualRetryResults",
        "retryDiagnostics",
        "volatileCount",
    ] {
        assert!(json.get(removed).is_none(), "Legacy field {removed}");
    }
    let receipt = &json["rawCapture"];
    assert_eq!(receipt["inboxId"], 42);
    assert_eq!(receipt["totalCount"], 7);
    assert_eq!(receipt["ackStatus"], "ACKNOWLEDGED");
    assert!(receipt["receivedAt"].is_string());
    assert!(receipt.get("rawRegisters").is_none());
    assert!(receipt.get("kind").is_none());
}

struct TestDirectory(PathBuf);

#[tokio::test]
async fn startup_initializes_settings_without_connecting_even_when_config_enabled() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dir = TestDirectory::new();
    let template = include_str!("../../../../as200_config.example.toml")
        .replacen("\nenabled = false", "\nenabled = true", 1)
        .replace(
            "192.168.1.5:502",
            &listener.local_addr().unwrap().to_string(),
        );
    std::fs::write(dir.0.join("as200_config.toml"), &template).unwrap();
    let state = WorkerState::default();
    state
        .clone()
        .initialize(Some(dir.0.clone()), DatabaseState::default())
        .await;
    assert!(!state.status.read().await.enabled);
    assert_eq!(state.status.read().await.state, "STOPPED");
    assert!(state.tasks.lock().await.is_empty());
    assert!(state.engine.lock().await.is_none());
    assert!(state.startup_config.lock().await.as_ref().unwrap().enabled);
    assert!(!dir.journal().exists() && !dir.journal().with_extension("lock").exists());
    assert_eq!(
        std::fs::read_to_string(dir.0.join("as200_config.toml")).unwrap(),
        template
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}

#[test]
fn successful_probe_clears_only_idle_network_error_not_ack_or_data_evidence() {
    let mut status = WorkerStatus {
        state: "ERROR".into(),
        message: "Lỗi Modbus TCP AS200: Không kết nối được TCP PLC.".into(),
        ..Default::default()
    };
    status.confirm_read_only_probe();
    assert_eq!(status.state, "STOPPED");
    assert!(!status.enabled);
    for fault in ["ack", "pause", "data", "active"] {
        let mut status = WorkerStatus {
            state: "ERROR".into(),
            message: "Lỗi Modbus TCP AS200: Không kết nối được TCP PLC.".into(),
            ..Default::default()
        };
        match fault {
            "ack" => status.ack_status = "RECONCILIATION_REQUIRED",
            "pause" => status.pause_reason = Some("RESET".into()),
            "data" => status.blocked_inbox_count = 1,
            _ => status.enabled = true,
        }
        status.confirm_read_only_probe();
        assert_eq!(status.state, "ERROR");
    }
}
impl TestDirectory {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "golf-regression-{}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("as200_config.toml"), "test fixture only").unwrap();
        std::fs::create_dir(path.join("local-runtime")).unwrap();
        Self(path)
    }
    fn journal(&self) -> PathBuf {
        self.0.join("local-runtime/capture.json")
    }
}
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let path = self.0.canonicalize().unwrap();
        let temp = std::env::temp_dir().canonicalize().unwrap();
        assert_eq!(path.parent(), Some(temp.as_path()));
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("golf-regression-")
        );
        std::fs::remove_dir_all(path).unwrap();
    }
}
fn config() -> SimulatorConfig {
    SimulatorConfig {
        mode: "modbus".into(),
        enabled: true,
        technical_setup_confirmed: true,
        allow_plc_connection: true,
        ack_enabled: true,
        ack_protocol: Some("as200".into()),
        source_key: "test-source".into(),
        confirmed_session_id: Some("test-session".into()),
        plc_heartbeat_timeout_ms: Some(5000),
        address: "127.0.0.1:1".into(),
        timeout_ms: 300,
        journal_path: Some("local-runtime/capture.json".into()),
        operator_sku: Some("SKU-FROZEN".into()),
        ..Default::default()
    }
}
fn engine(cfg: SimulatorConfig, dir: &TestDirectory, source_id: i64) -> Engine {
    Engine {
        config: cfg,
        source_id,
        state: WorkerState::default(),
        _lock: OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.journal().with_extension("lock"))
            .unwrap(),
        projection_due: Mutex::new(std::time::Instant::now()),
        projection_cursors: Mutex::new((0, 0)),
        published_gate: Mutex::new(()),
    }
}
fn progress(cfg: &SimulatorConfig) -> Progress {
    Progress {
        source_id: 1,
        session_id: "test-session".into(),
        binding: legacy::binding(cfg),
        accepted: 0,
        last_total: 1,
        anchor_inbox_id: None,
        pending_inbox_id: Some(2),
        ack_attempted: false,
        done_started: Some(false),
        done_observed: false,
        blocked_reason: None,
    }
}
async fn closed_pool() -> PgPool {
    let p = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .unwrap();
    p.close().await;
    p
}

// Loopback Modbus peer only; it emulates registers, not the Worker or DB logic.
struct Plc {
    address: String,
    words: Arc<Mutex<Vec<u16>>>,
    writes: Arc<Mutex<Vec<(u8, i32)>>>,
    drop_reply: Arc<Mutex<Option<u8>>>,
    commit_probe: Arc<Mutex<Option<(PgPool, i64)>>>,
    commit_trace: Arc<Mutex<Vec<serde_json::Value>>>,
    reads: Arc<Mutex<Vec<(u16, u16)>>>,
    delay_reply: Arc<Mutex<Option<u8>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Plc {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Plc {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let words = Arc::new(Mutex::new(vec![0_u16; 260]));
        {
            let mut w = words.lock().await;
            w[0] = 1;
            w[1] = 240;
            w[2] = 2;
        }
        let writes = Arc::new(Mutex::new(Vec::new()));
        let drop_reply = Arc::new(Mutex::new(None));
        let commit_probe: Arc<Mutex<Option<(PgPool, i64)>>> = Arc::new(Mutex::new(None));
        let commit_trace = Arc::new(Mutex::new(Vec::new()));
        let reads = Arc::new(Mutex::new(Vec::new()));
        let delay_reply = Arc::new(Mutex::new(None));
        let (trace, read_events, delayed) =
            (commit_trace.clone(), reads.clone(), delay_reply.clone());
        let durable = commit_probe.clone();
        let (memory, events, fault) = (words.clone(), writes.clone(), drop_reply.clone());
        let task = tokio::spawn(async move {
            let mut clients = tokio::task::JoinSet::new();
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (memory, events, fault) = (memory.clone(), events.clone(), fault.clone());
                let durable = durable.clone();
                let (trace, read_events, delayed) =
                    (trace.clone(), read_events.clone(), delayed.clone());
                clients.spawn(async move {
                    loop {
                        let mut header = [0_u8; 7];
                        if stream.read_exact(&mut header).await.is_err() {
                            break;
                        }
                        let len = usize::from(u16::from_be_bytes([header[4], header[5]])) - 1;
                        assert!((5..=246).contains(&len));
                        let mut pdu = vec![0; len];
                        stream.read_exact(&mut pdu).await.unwrap();
                        let start = usize::from(u16::from_be_bytes([pdu[1], pdu[2]]));
                        assert!((25000..25260).contains(&start));
                        let mut w = memory.lock().await;
                        let offset = start - 25000;
                        let response = match pdu[0] {
                            3 => {
                                let count = usize::from(u16::from_be_bytes([pdu[3], pdu[4]]));
                                assert!(count <= 100 && offset + count <= 260);
                                read_events.lock().await.push((start as u16, count as u16));
                                if offset == 0 {
                                    w[7] = (w[7] + 1) % 32768;
                                }
                                let mut out = vec![3, (count * 2) as u8];
                                for value in &w[offset..offset + count] {
                                    out.extend(value.to_be_bytes());
                                }
                                out
                            }
                            16 => {
                                assert_eq!(start, 25008);
                                assert_eq!(&pdu[3..6], &[0, 2, 4]);
                                w[8] = u16::from_be_bytes([pdu[6], pdu[7]]);
                                w[9] = u16::from_be_bytes([pdu[8], pdu[9]]);
                                let count = i32::from(w[8]) | (i32::from(w[9]) << 16);
                                if let Some((pool, source)) = durable.lock().await.as_ref() {
                                    let committed: Option<i64> = tokio::time::timeout(Duration::from_secs(2),
                                        sqlx::query_scalar("SELECT m.id FROM plc_inbox i JOIN measurements m ON m.id=i.measurement_id WHERE i.publication_count=$1 AND i.source_id=$2 AND m.source_id=i.source_id AND i.processing_status='PROCESSED' AND m.publication_count=i.publication_count")
                                            .bind(count).bind(source).fetch_optional(pool)).await.unwrap().unwrap();
                                    assert!(committed.is_some(), "FC16 arrived before Measurement COMMIT");
                                    trace.lock().await.push(serde_json::json!({"publicationCount":count,"measurementId":committed,"committedBeforeFc16":true}));
                                }
                                events.lock().await.push((16, count));
                                pdu[..5].to_vec()
                            }
                            6 => {
                                assert_eq!(start, 25012);
                                assert_eq!(&pdu[3..5], &[0, 1]);
                                w[12] = 1;
                                events.lock().await.push((6, 1));
                                pdu.clone()
                            }
                            other => panic!("unexpected FC {other}"),
                        };
                        drop(w);
                        let mut f = fault.lock().await;
                        if *f == Some(pdu[0]) {
                            *f = None;
                            break;
                        }
                        drop(f);
                        let mut wait = delayed.lock().await;
                        if *wait == Some(pdu[0]) {
                            *wait = None;
                            drop(wait);
                            tokio::time::sleep(Duration::from_millis(1500)).await;
                        } else { drop(wait); }
                        let mut out = header.to_vec();
                        out[4..6].copy_from_slice(&((response.len() + 1) as u16).to_be_bytes());
                        out.extend(response);
                        if stream.write_all(&out).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        Self {
            address,
            words,
            writes,
            drop_reply,
            commit_probe,
            commit_trace,
            reads,
            delay_reply,
            task,
        }
    }
    async fn publish(&self, total: i32, raw: &[u16]) {
        let mut w = self.words.lock().await;
        w[4] = total as u16;
        w[5] = (total as u32 >> 16) as u16;
        w[11] = 1;
        w[20..260].copy_from_slice(raw);
    }
    async fn complete(&self) {
        let mut w = self.words.lock().await;
        w[11] = 0;
        w[12] = 0;
    }
}

#[test]
fn telemetry_distinguishes_idle_unstable_transport_loss_and_vietnam_day() {
    let mut s = WorkerStatus::default();
    let before = DateTime::parse_from_rfc3339("2026-11-16T16:59:59Z")
        .unwrap()
        .with_timezone(&Utc);
    s.observe_read(before, None);
    s.connection = "CONNECTED".into();
    s.observe_read(before, Some(&As200Error::Unstable));
    assert_eq!(s.read_telemetry.as_ref().unwrap().read_errors, 0);
    s.observe_read(before, Some(&As200Error::Timeout));
    s.connection = "DISCONNECTED".into();
    s.observe_read(before, Some(&As200Error::Timeout));
    let t = s.read_telemetry.as_ref().unwrap();
    assert_eq!((t.read_errors, t.disconnects), (2, 1));
    s.observe_read(before + chrono::Duration::seconds(2), None);
    let t = s.read_telemetry.as_ref().unwrap();
    assert_eq!(t.date.to_string(), "2026-11-17");
    assert_eq!((t.read_errors, t.disconnects), (0, 0));
}
#[tokio::test]
async fn raw_receipt_display_uses_own_ack_identity_and_preserves_zero_unconfirmed() {
    let dir = TestDirectory::new();
    let e = engine(config(), &dir, 1);
    let mut p = progress(&e.config);
    let mut row = receipt(1);
    row.raw_registers = serde_json::json!(vec![0_u16; 240]);
    e.show_receipt(&row, &p).await.unwrap();
    assert_eq!(
        e.state
            .status
            .read()
            .await
            .raw_capture
            .as_ref()
            .unwrap()
            .ack_status,
        "PENDING"
    );
    for done in [Some(true), None] {
        p.ack_attempted = true;
        p.done_started = done;
        e.show_receipt(&row, &p).await.unwrap();
        assert_eq!(
            e.state
                .status
                .read()
                .await
                .raw_capture
                .as_ref()
                .unwrap()
                .ack_status,
            "RECONCILIATION_REQUIRED"
        );
    }
    p.accepted = 1;
    p.pending_inbox_id = None;
    p.anchor_inbox_id = Some(row.id);
    e.show_receipt(&row, &p).await.unwrap();
    assert_eq!(
        e.state
            .status
            .read()
            .await
            .raw_capture
            .as_ref()
            .unwrap()
            .ack_status,
        "ACKNOWLEDGED"
    );
    let mut zero = receipt(0);
    zero.received_at = row.received_at + chrono::Duration::seconds(1);
    zero.raw_registers = serde_json::json!(vec![0_u16; 240]);
    e.show_receipt(&zero, &p).await.unwrap();
    assert_eq!(
        e.state
            .status
            .read()
            .await
            .raw_capture
            .as_ref()
            .unwrap()
            .ack_status,
        "RECONCILIATION_REQUIRED"
    );
    assert!(e.state.status.read().await.last_saved_id.is_none());
}
#[tokio::test]
async fn projection_display_restores_measurement_link_and_never_overwrites_with_older_receipt() {
    let dir = TestDirectory::new();
    let e = engine(config(), &dir, 1);
    let mut p = progress(&e.config);
    let mut row = receipt(1);
    row.measurement_id = Some(42);
    row.processing_status = "PROCESSED".into();
    p.accepted = 1;
    p.pending_inbox_id = None;
    e.show_receipt(&row, &p).await.unwrap();
    let s = e.state.status.read().await;
    assert_eq!(s.last_saved_id, Some(42));
    assert_eq!(s.latest.as_ref().unwrap().save_state, "PROCESSED");
    assert_eq!(s.ack_status, "ACKNOWLEDGED");
    drop(s);
    let mut old = receipt(0);
    old.received_at = row.received_at - chrono::Duration::seconds(1);
    e.show_receipt(&old, &p).await.unwrap();
    assert_eq!(
        e.state
            .status
            .read()
            .await
            .latest
            .as_ref()
            .unwrap()
            .inbox_id,
        Some(row.id)
    );
}

#[tokio::test]
async fn committed_zero_publication_replaces_previous_completed_display() {
    let dir = TestDirectory::new();
    let e = engine(config(), &dir, 1);
    let mut p = progress(&e.config);
    let previous = receipt(1);
    e.show_receipt(&previous, &p).await.unwrap();
    let mut zero = receipt(2);
    zero.record_seq = 0;
    zero.raw_registers = serde_json::json!(vec![0_u16; 240]);
    zero.received_at = previous.received_at + chrono::Duration::seconds(1);
    zero.measurement_id = Some(43);
    zero.processing_status = "PROCESSED".into();
    p.pending_inbox_id = Some(zero.id);
    e.show_receipt(&zero, &p).await.unwrap();
    let status = e.state.status.read().await;
    let latest = status.latest.as_ref().unwrap();
    assert_eq!(latest.inbox_id, Some(zero.id));
    assert_eq!(latest.record_seq, 0);
    assert_eq!(latest.club_number, 0);
    assert_eq!(latest.overall_result, "UNMEASURED");
    assert!(latest.quality_error.is_some());
    assert_eq!(latest.save_state, "PROCESSED");
    assert_eq!(status.last_saved_id, Some(43));
    assert!(status.raw_capture.is_none());
}
#[tokio::test]
async fn database_unavailable_prevents_snapshot_and_any_ack_write() {
    let plc = Plc::new().await;
    let dir = TestDirectory::new();
    for required in [true, false] {
        let mut cfg = config();
        cfg.address = plc.address.clone();
        cfg.require_plc_heartbeat_for_ack = required;
        let e = engine(cfg, &dir, 1);
        assert!(
            e.poll_and_ack_published(&closed_pool().await)
                .await
                .is_err()
        );
        assert!(plc.writes.lock().await.is_empty());
        assert!(e.state.status.read().await.control_snapshot.is_none());
    }
}
#[tokio::test]
async fn live_idle_snapshot_is_connected_but_requires_heartbeat_change_before_writing() {
    let plc = Plc::new().await;
    let dir = TestDirectory::new();
    let mut cfg = config();
    cfg.address = plc.address.clone();
    let e = engine(cfg, &dir, 1);
    let snapshot = e.read_snapshot().await.unwrap();
    assert!(snapshot.buffer.is_none());
    assert_eq!(e.state.status.read().await.connection, "CONNECTED");
    assert!(!e.state.fresh_health().await);
    assert!(!e.state.heartbeat_allows_ack(true).await);
    assert!(e.state.heartbeat_allows_ack(false).await);
    assert_eq!(e.state.status.read().await.plc_health, "OBSERVING");
    e.read_snapshot().await.unwrap();
    assert!(e.state.fresh_health().await);
    assert!(e.state.heartbeat_allows_ack(true).await);
    assert!(e.state.heartbeat_allows_ack(false).await);
    assert!(e.state.status.read().await.warnings.is_empty());
    assert!(plc.writes.lock().await.is_empty());
}
#[tokio::test]
async fn ack_heartbeat_policy_preserves_monitoring_and_requires_connection() {
    use as200::heartbeat::HeartbeatMonitor;
    use std::time::Instant;
    for health in ["OBSERVING", "RUNNING", "STALE", "UNCONFIGURED"] {
        for required in [true, false] {
            let state = WorkerState::default();
            let now = Instant::now();
            let observed = if health == "STALE" {
                now - Duration::from_secs(60)
            } else {
                now
            };
            let mut monitor = HeartbeatMonitor::new(
                (health != "UNCONFIGURED").then_some(Duration::from_secs(30)),
                observed,
            );
            monitor.observe(1, observed);
            if health == "RUNNING" {
                monitor.observe(2, now);
            }
            *state.heartbeat.lock().unwrap() = Some(monitor);
            {
                let mut status = state.status.write().await;
                status.connection = "CONNECTED".into();
                status
                    .warnings
                    .push("MEASUREMENT: keep this warning".into());
                status.warnings.push("ACK_HEARTBEAT_BYPASS: old entry".into());
                status.warnings.push("RECONCILIATION_REQUIRED: keep this warning".into());
            }
            let bypass = !required && matches!(health, "OBSERVING" | "STALE");
            for _ in 0..2 {
                assert_eq!(
                    state.heartbeat_allows_ack(required).await,
                    health == "RUNNING" || bypass
                );
            }
            {
                let status = state.status.read().await;
                assert_eq!(status.plc_health, health);
                assert_eq!(status.warnings.len(), 2);
                assert!(status.warnings[0].starts_with("MEASUREMENT:"));
                assert!(status.warnings[1].starts_with("RECONCILIATION_REQUIRED:"));
            }
            state.status.write().await.connection = "DISCONNECTED".into();
            assert!(!state.heartbeat_allows_ack(required).await);
            assert_eq!(state.status.read().await.warnings.len(), 2);
        }
    }
    let state = WorkerState::default();
    state.status.write().await.connection = "CONNECTED".into();
    assert!(!state.heartbeat_allows_ack(false).await); // No monitor evidence.
}
#[test]
fn binding_changes_for_source_session_endpoint_unit_order_or_layout() {
    let base = config();
    let expected = legacy::binding(&base);
    for kind in 0..7 {
        let mut c = base.clone();
        match kind {
            0 => c.source_key = "other".into(),
            1 => c.confirmed_session_id = Some("other".into()),
            2 => c.address = "127.0.0.1:2".into(),
            3 => c.unit_id = 2,
            4 => c.word_order = crate::models::StoredWordOrder::High,
            5 => c.modbus_address_offset = -1,
            _ => c.control_start = 26000,
        };
        assert_ne!(legacy::binding(&c), expected);
    }
}
#[tokio::test]
async fn unsafe_legacy_evidence_is_rejected_without_database_network_or_file_rewrite() {
    let pool = closed_pool().await;
    for mutate in 0..9 {
        let dir = TestDirectory::new();
        let c = config();
        let path = dir.journal();
        let mut v = serde_json::json!({"version":4,"source_key":c.source_key,"session_id":"test-session","deliveries":[],"published_ack":{"binding":legacy::binding(&c),"accepted":0,"intent":null,"confirmed_receipt":null},"last_total":null});
        match mutate {
            0 => v["version"] = 3.into(),
            1 => v["session_id"] = "other".into(),
            2 => v["source_key"] = "other".into(),
            3 => v["published_ack"]["binding"] = "other".into(),
            4 => v["pause"] = "RESET".into(),
            5 => v["published_ack"]["intent"] = serde_json::json!({"phase":"DONE_WRITE_UNCERTAIN"}),
            6 => v["deliveries"] = serde_json::json!([{"unresolved":true}]),
            7 => v["quarantine"] = serde_json::json!([{}]),
            _ => v["last_total"] = 1.into(),
        }
        let before = serde_json::to_vec(&v).unwrap();
        std::fs::write(&path, &before).unwrap();
        let error = legacy::baseline(&c, &path, &pool, 1).await.unwrap_err();
        assert!(!error.contains("pool"), "must reject before DB: {error}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
#[tokio::test]
async fn stop_joins_task_and_keeps_durable_operator_marker_without_ack() {
    let dir = TestDirectory::new();
    let state = WorkerState::default();
    let file = dir.0.join("as200_config.toml");
    *state.settings_path.lock().await = Some(file.clone());
    state.status.write().await.enabled = true;
    let (tx, rx) = oneshot::channel();
    let stop = state.stopping.clone();
    state
        .tasks
        .lock()
        .await
        .push(tauri::async_runtime::spawn(async move {
            while !stop.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
            let _ = tx.send(());
        }));
    state.stop_collection().await.unwrap();
    rx.await.unwrap();
    assert!(file.with_extension("operator-stop").is_file());
    assert!(state.tasks.lock().await.is_empty());
    assert_eq!(state.status.read().await.state, "STOPPED");
    assert!(!dir.journal().exists());
}

async fn integrated(plc: &Plc, dir: &TestDirectory, pool: &PgPool) -> Arc<Engine> {
    let mut c = config();
    c.address = plc.address.clone();
    c.source_key = crate::database::test_support::source_key("worker");
    c.confirmed_session_id = Some(crate::database::test_support::session());
    c.operator_sku = Some(crate::database::test_support::sku());
    c.timeout_ms = 2000;
    c.plc_heartbeat_timeout_ms = Some(60000);
    assert!(
        c.address
            .parse::<std::net::SocketAddr>()
            .unwrap()
            .ip()
            .is_loopback()
    );
    Engine::open(c, dir.journal(), WorkerState::default(), pool)
        .await
        .unwrap()
}
async fn restart(e: Arc<Engine>, dir: &TestDirectory, pool: &PgPool) -> Arc<Engine> {
    let c = e.config.clone();
    drop(e);
    Engine::open(c, dir.journal(), WorkerState::default(), pool)
        .await
        .unwrap()
}
async fn poll(e: &Engine, pool: &PgPool) {
    let _ = e.poll_and_ack_published(pool).await;
}
async fn finish(e: &Engine, plc: &Plc, pool: &PgPool, target: i32) {
    poll(e, pool).await; // persist Done high, if first write already observed it this remains safe
    assert!(
        store::progress(pool, e.source_id)
            .await
            .unwrap()
            .unwrap()
            .done_observed
    );
    plc.complete().await;
    poll(e, pool).await;
    let p = store::progress(pool, e.source_id).await.unwrap().unwrap();
    assert_eq!(p.accepted, target);
    assert!(p.pending_inbox_id.is_none());
}

#[tokio::test]
#[ignore = "Dedicated loopback golf_report_migration_test only; idle preparation and durable conflict regression"]
async fn local_idle_preparation_does_not_latch_counters_or_release_pending_conflicts() {
    let pool = crate::database::migrations::integration_tests::isolated().await;
    crate::database::migrations::migrate(&pool).await.unwrap();
    for fault in ["ready-low", "payload", "target"] {
        let plc = Plc::new().await;
        let dir = TestDirectory::new();
        let mut cfg = config();
        cfg.address = plc.address.clone();
        cfg.source_key = format!("idle-preparation-{fault}");
        cfg.timeout_ms = 2000;
        // Startup itself can occur during preparation of an unpublished record.
        plc.words.lock().await[4] = 9;
        let e = Engine::open(cfg, dir.journal(), WorkerState::default(), &pool)
            .await
            .unwrap();
        e.poll_and_ack_published(&pool).await.unwrap();
        let p = store::progress(&pool, e.source_id).await.unwrap().unwrap();
        assert_eq!((p.accepted, p.last_total), (0, 0));
        assert!(p.blocked_reason.is_none() && p.pending_inbox_id.is_none());

        plc.publish(1, &registers(1)).await;
        poll(&e, &pool).await;
        finish(&e, &plc, &pool, 1).await;
        let writes = plc.writes.lock().await.clone();
        let p = store::progress(&pool, e.source_id).await.unwrap().unwrap();
        let anchor = p.anchor_inbox_id;
        for total in [9, 0, 2] {
            {
                let mut w = plc.words.lock().await;
                w[4] = total;
                w[22] = 55; // Changing StickNo and RecordSeq is still preparation.
                w[42] = 77;
                w[151] ^= 1;
            }
            plc.reads.lock().await.clear();
            e.poll_and_ack_published(&pool).await.unwrap();
            assert!(plc.reads.lock().await.iter().all(|r| *r == (25000, 20)));
            let p = store::progress(&pool, e.source_id).await.unwrap().unwrap();
            assert_eq!((p.accepted, p.last_total), (1, 1));
            assert_eq!(p.anchor_inbox_id, anchor);
            assert!(p.pending_inbox_id.is_none() && p.blocked_reason.is_none());
            assert!(!p.ack_attempted && !p.done_observed);
            assert_eq!(p.done_started, Some(false));
            assert_eq!(*plc.writes.lock().await, writes);
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM plc_inbox WHERE source_id=$1")
            .bind(e.source_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        let e = restart(e, &dir, &pool).await;
        e.poll_and_ack_published(&pool).await.unwrap();

        // Reproduce PROCESSED + pending receipt before the first ACK attempt.
        let raw = registers(2);
        plc.publish(2, &raw).await;
        let snapshot = e.read_snapshot().await.unwrap();
        let p = store::progress(&pool, e.source_id).await.unwrap().unwrap();
        let row = e.receive(&pool, &p, &snapshot, None).await.unwrap();
        e.process_receipt(&pool, &row).await.unwrap();
        assert_eq!(
            InboxRepository::find(&pool, row.id)
                .await
                .unwrap()
                .processing_status,
            "PROCESSED"
        );
        {
            let mut w = plc.words.lock().await;
            match fault {
                "ready-low" => w[11] = 0,
                "payload" => w[151] ^= 1,
                _ => w[4] = 3,
            }
        }
        let error = e.poll_and_ack_published(&pool).await.unwrap_err();
        assert!(error.contains("Raw/target"));
        if fault == "ready-low" {
            assert!(error.contains("DataReady về 0"));
        }
        let p = store::progress(&pool, e.source_id).await.unwrap().unwrap();
        assert_eq!(p.accepted, 1);
        assert_eq!(p.pending_inbox_id, Some(row.id));
        assert_eq!(p.blocked_reason.as_deref(), Some("PUBLICATION_CONFLICT"));
        assert!(!p.ack_attempted && !p.done_observed);
        assert_eq!(p.done_started, Some(false));
        assert_eq!(
            InboxRepository::find(&pool, row.id)
                .await
                .unwrap()
                .raw_registers,
            serde_json::json!(raw)
        );
        assert_eq!(*plc.writes.lock().await, writes);

        // Matching the receipt again, even after restart, cannot clear the latch.
        plc.publish(2, &raw).await;
        let e = restart(e, &dir, &pool).await;
        assert!(
            e.poll_and_ack_published(&pool)
                .await
                .unwrap_err()
                .contains("PUBLICATION_CONFLICT")
        );
        assert_eq!(*plc.writes.lock().await, writes);
        assert_eq!(
            store::progress(&pool, e.source_id)
                .await
                .unwrap()
                .unwrap()
                .blocked_reason
                .as_deref(),
            Some("PUBLICATION_CONFLICT")
        );
    }
    pool.close().await;
}

#[tokio::test]
#[ignore = "Dedicated loopback golf_report_migration_test only; isolated schema, never Neon"]
async fn local_manual_measurement_commit_precedes_ack_and_retry_is_idempotent() {
    let pool = crate::database::migrations::integration_tests::isolated().await;
    crate::database::migrations::migrate(&pool).await.unwrap();
    let plc = Plc::new().await;
    let dir = TestDirectory::new();
    let mut cfg = config();
    cfg.timeout_ms = 2000;
    cfg.address = plc.address.clone();
    cfg.source_key = "manual-loopback-only".into();
    cfg.operator_sku = Some("MANUAL-QC".into());
    let e = Engine::open(cfg, dir.journal(), WorkerState::default(), &pool)
        .await
        .unwrap();
    *plc.commit_probe.lock().await = Some((pool.clone(), e.source_id));
    let raw = vec![0_u16; 240];
    // Fail COMMIT itself, after the tree and Inbox link have been written inside
    // the transaction. This deferred trigger is confined to the local test schema.
    sqlx::raw_sql("CREATE FUNCTION manual_commit_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.publication_count=1 THEN RAISE EXCEPTION 'manual test: deferred Measurement COMMIT rejected'; END IF; RETURN NEW; END $$; CREATE CONSTRAINT TRIGGER manual_fault AFTER INSERT ON measurements DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION manual_commit_fault()")
    .execute(&pool)
    .await
    .unwrap();
    plc.publish(1, &raw).await;
    assert!(e.poll_and_ack_published(&pool).await.is_err());
    assert!(plc.writes.lock().await.is_empty());
    let p = store::progress(&pool, e.source_id).await.unwrap().unwrap();
    assert!(!p.ack_attempted);
    let first = InboxRepository::find(&pool, p.pending_inbox_id.unwrap())
        .await
        .unwrap();
    assert_eq!(first.raw_registers, serde_json::json!(raw));
    assert!(first.measurement_id.is_none());
    assert_eq!(first.processing_status, "FAILED");
    assert!(
        first
            .error_message
            .as_ref()
            .unwrap()
            .contains("deferred Measurement COMMIT rejected")
    );
    let trees: i64 = sqlx::query_scalar("SELECT count(*) FROM measurements")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(trees, 0);
    sqlx::raw_sql("DROP TRIGGER manual_fault ON measurements; DROP FUNCTION manual_commit_fault()")
        .execute(&pool)
        .await
        .unwrap();
    let mut stages = Vec::new();
    let schema: String = sqlx::query_scalar("SELECT current_schema()::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    let source_key = e.config.source_key.clone();
    for count in 1..=5 {
        let mut words = raw.clone();
        match count {
            2 => {
                words[2] = 12;
                words[129] = 1;
                for (offset, value) in [(132, 10_f32), (134, 12_f32), (136, 0_f32)] {
                    words[offset] = value.to_bits() as u16;
                    words[offset + 1] = (value.to_bits() >> 16) as u16;
                }
            }
            4 => words[131] = 12,
            5 => {
                words[0] = 1;
                words[1] = 1;
                words[2] = 55;
                words[4] = 0x4241;
                words[20] = 1;
                words[22] = 5;
                for (header, base, points) in [(24, 32, 4), (120, 128, 1)] {
                    words[header] = 1;
                    words[header + 1] = points;
                    for point in 0..usize::from(points) {
                        let offset = base + point * 22;
                        words[offset] = 1;
                        words[offset + 1] = 1;
                        words[offset + 2] = 1;
                    }
                }
            }
            _ => {}
        }
        plc.publish(count, &words).await;
        poll(&e, &pool).await;
        let p = store::progress(&pool, e.source_id).await.unwrap().unwrap();
        let row = InboxRepository::find(&pool, p.pending_inbox_id.unwrap())
            .await
            .unwrap();
        let id = row
            .measurement_id
            .expect("Measurement must commit before Count/Done");
        let detail = crate::repositories::MeasurementRepository::find_by_id(&pool, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail.summary.record_seq, if count == 5 { 5 } else { 0 });
        assert_eq!(detail.summary.publication_count, Some(count));
        assert_eq!(
            detail.summary.club_number,
            match count {
                2 => 12,
                5 => 55,
                _ => 0,
            }
        );
        assert_eq!(detail.summary.product_code, "MANUAL-QC");
        assert_eq!(
            detail.summary.overall_result,
            if count == 5 {
                crate::enums::MeasurementResult::Ok
            } else {
                crate::enums::MeasurementResult::Unmeasured
            }
        );
        assert_eq!(detail.summary.quality_error.is_some(), count != 5);
        assert_eq!(detail.receipt.as_ref().unwrap().inbox_id, row.id);
        if count == 1 {
            assert!(
                detail
                    .stations
                    .iter()
                    .all(|s| s.result == crate::enums::MeasurementResult::Unmeasured
                        && s.points.is_empty())
            );
        }
        if count == 2 {
            let point = &detail
                .stations
                .iter()
                .find(|s| s.station_type == crate::models::StationType::Tip)
                .unwrap()
                .points[0];
            assert_eq!(point.position.as_ref().unwrap().to_string(), "10");
            assert_eq!(point.attempt_count, 1);
            assert_eq!(point.decide_attempt, None);
            assert_eq!(point.result, crate::enums::MeasurementResult::Unmeasured);
            assert_eq!(
                point.attempts[0].push_dist.as_ref().unwrap().to_string(),
                "12"
            );
            assert_eq!(point.attempts[0].bend.as_ref().unwrap().to_string(), "0");
        }
        assert_eq!(row.raw_registers, serde_json::json!(words));
        assert_eq!(InboxService::process(&pool, row.id).await.unwrap(), id);
        finish(&e, &plc, &pool, count).await;
        let request = crate::models::MeasurementListRequest {
            page_size: Some(100),
            product_code: e.config.operator_sku.clone(),
            ..Default::default()
        };
        let history = crate::repositories::MeasurementRepository::history_page(&pool, &request)
            .await
            .unwrap();
        assert_eq!(history.total, i64::from(count));
        let mut details = Vec::new();
        for item in &history.items {
            details.push(
                crate::repositories::MeasurementRepository::find_by_id(&pool, item.summary.id)
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        let snapshot = serde_json::json!({
            "count":count,"status":e.state.status.read().await.clone(),"history":history,"measurements":details,
            "statistics":crate::repositories::MeasurementRepository::statistics(&pool,&request).await.unwrap(),
            "skuSummary":crate::repositories::MeasurementRepository::sku_summary(&pool,&request).await.unwrap(),
            "quality":crate::repositories::MeasurementRepository::quality_statistics(&pool,&request).await.unwrap()
        });
        stages.push(snapshot);
        let before = plc.writes.lock().await.len();
        plc.publish(count, &words).await; // Held Ready / duplicate identity.
        for _ in 0..5 {
            poll(&e, &pool).await;
        }
        assert_eq!(plc.writes.lock().await.len(), before);
        let (inbox_count,measurement_count):(i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM plc_inbox WHERE source_id=$1),(SELECT count(*) FROM measurements WHERE source_id=$1)")
            .bind(e.source_id).fetch_one(&pool).await.unwrap();
        assert_eq!(
            (inbox_count, measurement_count),
            (i64::from(count), i64::from(count))
        );
        plc.complete().await;
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM measurements WHERE source_id=$1")
        .bind(e.source_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 5);
    let request = crate::models::MeasurementListRequest::default();
    let stats = crate::repositories::MeasurementRepository::statistics(&pool, &request)
        .await
        .unwrap();
    assert_eq!(
        (stats.total_measurements, stats.total_ok, stats.total_ng),
        (1, 1, 0)
    );
    assert_eq!(stats.pass_rate, 100.0);
    assert_eq!(
        crate::repositories::MeasurementRepository::list(&pool, &request)
            .await
            .unwrap()
            .total,
        5
    );
    assert_eq!(
        crate::repositories::MeasurementRepository::quality_statistics(&pool, &request)
            .await
            .unwrap()
            .points
            .len(),
        5
    );
    assert_eq!(
        *plc.writes.lock().await,
        vec![
            (16, 1),
            (6, 1),
            (16, 2),
            (6, 1),
            (16, 3),
            (6, 1),
            (16, 4),
            (6, 1),
            (16, 5),
            (6, 1)
        ]
    );
    let page2 = crate::repositories::MeasurementRepository::history_page(
        &pool,
        &crate::models::MeasurementListRequest {
            page: Some(2),
            page_size: Some(2),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(page2.total, 5);
    assert_eq!(page2.items.len(), 2);
    let reads = plc.reads.lock().await.clone();
    for chunk in [(25020, 100), (25120, 100), (25220, 40)] {
        assert!(reads.contains(&chunk));
    }
    let missing_sku = crate::repositories::MeasurementRepository::history_page(
        &pool,
        &crate::models::MeasurementListRequest {
            product_code: Some("NOT-THIS-SKU".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(missing_sku.total, 0);
    let evidence = serde_json::json!({"database":"golf_report_migration_test","schema":schema,"sourceKey":source_key,"session":e.config.confirmed_session_id,"endpoint":plc.address,"stages":stages,"fc03":reads,"writes":plc.writes.lock().await.clone(),"commitProof":plc.commit_trace.lock().await.clone(),"paginationPage2":page2,"measurementCommitFailure":{"inboxId":first.id,"status":first.processing_status,"error":first.error_message,"measurementCount":trees,"ackAttempted":p.ack_attempted}});
    if let Ok(directory) = std::env::var("GOLF_MANUAL_EVIDENCE_DIR") {
        let directory = std::path::PathBuf::from(directory).canonicalize().unwrap();
        assert!(directory.starts_with(std::env::temp_dir().canonicalize().unwrap()));
        std::fs::write(
            directory.join("manual-evidence.json"),
            serde_json::to_vec_pretty(&evidence).unwrap(),
        )
        .unwrap();
    }
    println!(
        "MANUAL_SQL_EVIDENCE {}",
        serde_json::json!({"schema":schema,"sourceKey":source_key,"measurements":5,"accepted":5})
    );
    drop(e);
    pool.close().await;
}

#[tokio::test]
#[ignore = "Dedicated loopback golf_report_migration_test only; real SQL and network faults"]
async fn local_manual_restart_and_lost_ack_responses_do_not_replay_writes() {
    let pool = crate::database::migrations::integration_tests::isolated().await;
    crate::database::migrations::migrate(&pool).await.unwrap();
    let schema: String = sqlx::query_scalar("SELECT current_schema()::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut evidence = Vec::new();
    for fault in 0..3 {
        let plc = Plc::new().await;
        let dir = TestDirectory::new();
        let mut cfg = config();
        cfg.address = plc.address.clone();
        cfg.source_key = format!("manual-recovery-{fault}");
        cfg.timeout_ms = 300;
        cfg.plc_heartbeat_timeout_ms = Some(60000);
        let mut e = Engine::open(cfg, dir.journal(), WorkerState::default(), &pool)
            .await
            .unwrap();
        *plc.commit_probe.lock().await = Some((pool.clone(), e.source_id));
        let raw = vec![0_u16; 240];
        plc.publish(1, &raw).await;
        if fault == 0 {
            // Real receipt/Measurement commit, then disconnect during the next
            // FC03, before any write. Restart must hydrate that same link.
            let snapshot = e.read_snapshot().await.unwrap();
            let p = store::progress(&pool, e.source_id).await.unwrap().unwrap();
            let row = e.receive(&pool, &p, &snapshot, None).await.unwrap();
            e.process_receipt(&pool, &row).await.unwrap();
            *plc.drop_reply.lock().await = Some(3);
        } else if fault == 1 {
            *plc.delay_reply.lock().await = Some(16); // actual response timeout
        } else {
            *plc.drop_reply.lock().await = Some(6); // Done received, response lost
        }
        let error = e.poll_and_ack_published(&pool).await.unwrap_err();
        let prior_writes = plc.writes.lock().await.clone();
        if fault == 0 {
            assert!(prior_writes.is_empty());
        }
        if fault == 1 {
            assert_eq!(prior_writes, vec![(16, 1)]);
        }
        if fault == 2 {
            assert_eq!(prior_writes, vec![(16, 1), (6, 1)]);
        }
        let source = e.source_id;
        let row: InboxRecord = sqlx::query_as("SELECT * FROM plc_inbox WHERE source_id=$1")
            .bind(source)
            .fetch_one(&pool)
            .await
            .unwrap();
        let id = row.measurement_id.unwrap();
        e = restart(e, &dir, &pool).await;
        assert_eq!(e.state.status.read().await.last_saved_id, Some(id));
        poll(&e, &pool).await;
        finish(&e, &plc, &pool, 1).await;
        assert_eq!(*plc.writes.lock().await, vec![(16, 1), (6, 1)]);
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM measurements WHERE source_id=$1")
            .bind(source)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            store::progress(&pool, source)
                .await
                .unwrap()
                .unwrap()
                .accepted,
            1
        );
        evidence.push(serde_json::json!({"fault":fault,"error":error,"sourceId":source,"inboxId":row.id,"measurementId":id,"beforeRestart":prior_writes,"writes":plc.writes.lock().await.clone(),"accepted":1}));
    }
    if let Ok(directory) = std::env::var("GOLF_MANUAL_EVIDENCE_DIR") {
        let directory = PathBuf::from(directory).canonicalize().unwrap();
        assert!(directory.starts_with(std::env::temp_dir().canonicalize().unwrap()));
        std::fs::write(
            directory.join("recovery-evidence.json"),
            serde_json::to_vec_pretty(&serde_json::json!({"schema":schema,"cases":evidence}))
                .unwrap(),
        )
        .unwrap();
    }
    pool.close().await;
}
#[tokio::test]
#[ignore = "Requires explicit verified Neon integration opt-in; exercises real production SQL and loopback PLC"]
async fn postgres_raw_zero_one_word_counter_zero_to_one_and_distinct_identical_publications() {
    let pool = crate::database::test_support::pool(3).await;
    let plc = Plc::new().await;
    let dir = TestDirectory::new();
    let e = integrated(&plc, &dir, &pool).await;
    let raw = vec![0_u16; 240];
    plc.publish(0, &raw).await;
    poll(&e, &pool).await;
    let zero = store::progress(&pool, e.source_id).await.unwrap().unwrap();
    assert_eq!(zero.accepted, 0);
    assert!(!zero.ack_attempted);
    assert!(plc.writes.lock().await.is_empty());
    let e = restart(e, &dir, &pool).await;
    for target in 1..=2 {
        plc.publish(target, &raw).await;
        poll(&e, &pool).await;
        finish(&e, &plc, &pool, target).await;
    }
    let mut one = vec![0_u16; 240];
    one[180] = 12;
    plc.publish(3, &one).await;
    poll(&e, &pool).await;
    finish(&e, &plc, &pool, 3).await;
    let rows: Vec<InboxRecord> =
        sqlx::query_as("SELECT * FROM plc_inbox WHERE source_id=$1 ORDER BY publication_count")
            .bind(e.source_id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].publication_count, Some(0));
    assert_eq!(rows[1].raw_registers, rows[2].raw_registers);
    assert_ne!(rows[1].id, rows[2].id);
    assert!(
        rows.iter().all(|r| r.record_seq == 0)
            && rows[1..].iter().all(|r| r.measurement_id.is_some())
    );
    assert_eq!(
        *plc.writes.lock().await,
        vec![(16, 1), (6, 1), (16, 2), (6, 1), (16, 3), (6, 1)]
    );
    e.project_pending(&pool).await.unwrap();
    assert!(
        InboxRepository::find(&pool, rows[3].id)
            .await
            .unwrap()
            .measurement_id
            .is_some()
    );
    let e = restart(e, &dir, &pool).await;
    assert_eq!(e.state.status.read().await.ack_status, "ACKNOWLEDGED");
    assert_eq!(
        e.state.status.read().await.last_saved_id,
        rows[3].measurement_id
    );
    poll(&e, &pool).await;
    assert_eq!(plc.writes.lock().await.len(), 6);
}
#[tokio::test]
#[ignore = "Requires explicit verified Neon integration opt-in; restart and lost-response SQL/ACK integration"]
async fn postgres_count_loss_first_done_restart_and_unknown_done_never_resends() {
    let pool = crate::database::test_support::pool(3).await;
    let plc = Plc::new().await;
    let dir = TestDirectory::new();
    let e = integrated(&plc, &dir, &pool).await;
    plc.publish(1, &vec![0; 240]).await;
    *plc.drop_reply.lock().await = Some(16);
    poll(&e, &pool).await;
    let p = store::progress(&pool, e.source_id).await.unwrap().unwrap();
    assert!(p.ack_attempted);
    assert_eq!(p.done_started, Some(false));
    assert_eq!(*plc.writes.lock().await, vec![(16, 1)]);
    let e = restart(e, &dir, &pool).await;
    *plc.drop_reply.lock().await = Some(6);
    poll(&e, &pool).await;
    assert_eq!(*plc.writes.lock().await, vec![(16, 1), (6, 1)]);
    let e = restart(e, &dir, &pool).await;
    poll(&e, &pool).await;
    finish(&e, &plc, &pool, 1).await;
    // Old attempted row with NULL Done evidence is never treated as definitely unsent.
    plc.publish(2, &vec![0; 240]).await;
    let s = e.read_snapshot().await.unwrap();
    let p = store::progress(&pool, e.source_id).await.unwrap().unwrap();
    e.receive(&pool, &p, &s, None).await.unwrap();
    sqlx::query(
        "UPDATE plc_acquisition SET ack_attempted=true,done_started=NULL WHERE source_id=$1",
    )
    .bind(e.source_id)
    .execute(&pool)
    .await
    .unwrap();
    let e = restart(e, &dir, &pool).await;
    poll(&e, &pool).await;
    poll(&e, &pool).await;
    let p = store::progress(&pool, e.source_id).await.unwrap().unwrap();
    assert_eq!(p.accepted, 1);
    assert_eq!(p.done_started, None);
    assert_eq!(plc.writes.lock().await.len(), 2);
}
#[tokio::test]
#[ignore = "Requires explicit verified Neon integration opt-in; real projection, retry and batch restart"]
async fn postgres_one_two_five_ten_measurements_retry_and_restart() {
    let pool = crate::database::test_support::pool(3).await;
    let plc = Plc::new().await;
    let dir = TestDirectory::new();
    let mut e = integrated(&plc, &dir, &pool).await;
    for target in 1..=10 {
        plc.publish(target, &registers(target)).await;
        poll(&e, &pool).await;
        finish(&e, &plc, &pool, target).await;
        *e.projection_due.lock().await = std::time::Instant::now();
        e.project_pending(&pool).await.unwrap();
        if [1, 2, 5, 10].contains(&target) {
            let count: i64 =
                sqlx::query_scalar("SELECT count(*) FROM measurements WHERE source_id=$1")
                    .bind(e.source_id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(count, i64::from(target));
        }
        if target == 5 {
            e = restart(e, &dir, &pool).await;
        }
    }
    let p = store::progress(&pool, e.source_id).await.unwrap().unwrap();
    let id = p.anchor_inbox_id.unwrap();
    let a = InboxService::process(&pool, id).await.unwrap();
    assert_eq!(InboxService::process(&pool, id).await.unwrap(), a);
    let e = restart(e, &dir, &pool).await;
    assert_eq!(e.state.status.read().await.last_saved_id, Some(a));
    let request = crate::models::MeasurementListRequest {
        product_code: Some(crate::database::test_support::sku()),
        ..Default::default()
    };
    // Count source rows directly; history/read-only query is also exercised with a frozen SKU.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM measurements WHERE source_id=$1")
        .bind(e.source_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 10);
    let _ = crate::repositories::MeasurementRepository::history_page(&pool, &request)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "Requires explicit verified Neon integration opt-in; real permanent SQL conflicts and fair projection"]
async fn postgres_permanent_projection_failures_do_not_starve_new_measurement() {
    let pool = crate::database::test_support::pool(3).await;
    let plc = Plc::new().await;
    let dir = TestDirectory::new();
    let e = integrated(&plc, &dir, &pool).await;
    // Completed trees already exist; native Inbox projections for those identities fail UNIQUE.
    // Only test-owned rows are inserted. No triggers, schema edits or row deletions.
    for seq in 101..=112 {
        let mut row = receipt(seq);
        row.source_id = e.source_id;
        row.publication_session = e.config.confirmed_session_id.clone();
        row.assigned_sku = e.config.operator_sku.clone();
        let draft = crate::services::inbox_service::draft_from_inbox(&row).unwrap();
        let mut tx = pool.begin().await.unwrap();
        crate::services::MeasurementService::insert(&mut tx, &draft)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        sqlx::query("INSERT INTO plc_inbox(source_id,ingest_key,record_seq,total_count,buffer_index,raw_registers,word_order,received_at,processing_status,publication_session,publication_count,assigned_sku) VALUES($1,$2,$3,$3,0,$4,'LOW',now(),'PENDING',$5,$3,$6)")
            .bind(e.source_id).bind(format!("failed-fixture-{seq}")).bind(seq).bind(serde_json::to_value(registers(seq)).unwrap()).bind(&e.config.confirmed_session_id).bind(&e.config.operator_sku).execute(&pool).await.unwrap();
    }
    let good:i64=sqlx::query_scalar("INSERT INTO plc_inbox(source_id,ingest_key,record_seq,total_count,buffer_index,raw_registers,word_order,received_at,processing_status,publication_session,publication_count,assigned_sku) VALUES($1,'new-fixture',200,200,0,$2,'LOW',now(),'PENDING',$3,200,$4) RETURNING id")
        .bind(e.source_id).bind(serde_json::to_value(registers(200)).unwrap()).bind(&e.config.confirmed_session_id).bind(&e.config.operator_sku).fetch_one(&pool).await.unwrap();
    let e = restart(e, &dir, &pool).await;
    for _ in 0..5 {
        *e.projection_due.lock().await = std::time::Instant::now();
        e.project_pending(&pool).await.unwrap();
    }
    assert!(
        InboxRepository::find(&pool, good)
            .await
            .unwrap()
            .measurement_id
            .is_some()
    );
    let failed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM plc_inbox WHERE source_id=$1 AND processing_status='FAILED'",
    )
    .bind(e.source_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(failed, 12);
    assert_eq!(e.state.status.read().await.blocked_inbox_count, 12);
}

#[tokio::test]
#[ignore = "Requires explicit verified Neon integration opt-in; recovered unknown ACK evidence, no synthetic completion"]
async fn postgres_restart_with_count_uncertain_or_done_unknown_never_replays_or_invents_proof() {
    let pool = crate::database::test_support::pool(3).await;
    for done in [Some(false), Some(true), None] {
        let plc = Plc::new().await;
        let dir = TestDirectory::new();
        let e = integrated(&plc, &dir, &pool).await;
        plc.publish(1, &vec![0; 240]).await;
        let snap = e.read_snapshot().await.unwrap();
        let p = store::progress(&pool, e.source_id).await.unwrap().unwrap();
        e.receive(&pool, &p, &snap, None).await.unwrap();
        // A restored durable fixture: Count may have been sent; no Done-high proof.
        sqlx::query(
            "UPDATE plc_acquisition SET ack_attempted=true,done_started=$2 WHERE source_id=$1",
        )
        .bind(e.source_id)
        .bind(done)
        .execute(&pool)
        .await
        .unwrap();
        if done != Some(false) {
            let mut w = plc.words.lock().await;
            w[8] = 1;
            w[11] = 0;
        }
        let e = restart(e, &dir, &pool).await;
        poll(&e, &pool).await;
        poll(&e, &pool).await;
        let p = store::progress(&pool, e.source_id).await.unwrap().unwrap();
        assert_eq!(p.accepted, 0);
        assert!(!p.done_observed);
        assert!(p.pending_inbox_id.is_some());
        assert!(plc.writes.lock().await.is_empty());
    }
}
#[tokio::test]
#[ignore = "Requires explicit verified Neon integration opt-in; real raw conflict, gap, reset and binding gates"]
async fn postgres_gap_changed_payload_reset_and_binding_do_not_ack_wrong_publication() {
    let pool = crate::database::test_support::pool(3).await;
    for fault in ["gap", "payload", "reset", "binding"] {
        let plc = Plc::new().await;
        let dir = TestDirectory::new();
        let e = integrated(&plc, &dir, &pool).await;
        if fault == "gap" {
            plc.publish(2, &vec![0; 240]).await;
            poll(&e, &pool).await;
        } else {
            plc.publish(1, &vec![0; 240]).await;
            let snap = e.read_snapshot().await.unwrap();
            let p = store::progress(&pool, e.source_id).await.unwrap().unwrap();
            let row = e.receive(&pool, &p, &snap, None).await.unwrap();
            match fault {
                "payload" => {
                    let mut raw = vec![0; 240];
                    raw[180] = 12;
                    plc.publish(1, &raw).await;
                }
                "reset" => plc.publish(0, &vec![0; 240]).await,
                _ => {
                    sqlx::query("UPDATE plc_acquisition SET binding='incompatible-fixture' WHERE source_id=$1").bind(e.source_id).execute(&pool).await.unwrap();
                }
            }
            assert!(e.poll_and_ack_published(&pool).await.is_err());
            assert_eq!(
                InboxRepository::find(&pool, row.id)
                    .await
                    .unwrap()
                    .raw_registers,
                serde_json::json!(vec![0; 240])
            );
        }
        let p = store::progress(&pool, e.source_id).await.unwrap().unwrap();
        assert_eq!(p.accepted, 0);
        assert!(!p.ack_attempted);
        assert!(plc.writes.lock().await.is_empty());
        if ["payload", "reset"].contains(&fault) {
            assert!(p.blocked_reason.is_some());
        }
    }
}
#[tokio::test]
#[ignore = "Requires explicit verified Neon integration opt-in; read-only historical cutover proofs"]
async fn postgres_legacy_sentinel_and_next_total_require_real_anchor_and_complete_prefix() {
    let pool = crate::database::test_support::pool(3).await;
    let dir = TestDirectory::new();
    let mut c = config();
    c.source_key = crate::database::test_support::source_key("legacy");
    c.confirmed_session_id = Some(crate::database::test_support::session());
    let session = c.confirmed_session_id.as_ref().unwrap();
    let source = SourceRepository::upsert(&pool, &c.source_key, None)
        .await
        .unwrap();
    let key = format!("raw-published-v1:ack:{}:{session}:1", session.len());
    let row:InboxRecord=sqlx::query_as("INSERT INTO plc_inbox(source_id,ingest_key,record_seq,total_count,buffer_index,raw_registers,word_order,received_at,processing_status) VALUES($1,$2,0,1,0,$3,'LOW',now(),'SESSION_UNCONFIRMED') RETURNING *")
        .bind(source.id).bind(&key).bind(serde_json::json!(vec![0;240])).fetch_one(&pool).await.unwrap();
    let mut v = serde_json::json!({"version":4,"source_key":c.source_key,"session_id":session,"last_total":2,"deliveries":[],
        "published_ack":{"binding":legacy::binding(&c),"accepted":1,
            "confirmed_receipt":{"inbox_id":row.id,"input":{"source_id":0,"confirmed_session_id":null,"ingest_key":key,"record_seq":0,"total_count":1,"buffer_index":0,"raw_registers":vec![0;240],"word_order":"LOW","received_at":row.received_at}},
            "intent":{"phase":"ACK_CONFIRMED","target":1,"inbox_id":row.id,"raw_registers":vec![0;240],"observed_done_high":true}}});
    let path = dir.journal();
    let bytes = serde_json::to_vec(&v).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(
        legacy::baseline(&c, &path, &pool, source.id).await.unwrap(),
        (1, Some(row.id), 2)
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    // A mismatched source ID or an incomplete prefix is not normalized or repaired.
    v["published_ack"]["confirmed_receipt"]["input"]["source_id"] = (source.id + 1).into();
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    assert!(legacy::baseline(&c, &path, &pool, source.id).await.is_err());
    v["published_ack"]["confirmed_receipt"]["input"]["source_id"] = 0.into();
    v["published_ack"]["accepted"] = 2.into();
    v["last_total"] = 2.into();
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    assert!(legacy::baseline(&c, &path, &pool, source.id).await.is_err());
}
