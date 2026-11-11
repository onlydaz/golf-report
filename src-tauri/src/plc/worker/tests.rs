use super::*;

#[tokio::test]
async fn operator_stop_waits_for_tasks_and_survives_restart_without_journal_changes() {
    let dir = TestDirectory::new();
    let file = dir.0.join("as200_config.toml");
    std::fs::write(&file, "enabled = true\n").unwrap();
    let state = WorkerState::default();
    *state.settings_path.lock().await = Some(file.clone());
    state.status.write().await.enabled = true;
    let finished = Arc::new(AtomicBool::new(false));
    let done = finished.clone();
    let stop = state.stopping.clone();
    state.tasks.lock().await.push(tauri::async_runtime::spawn(async move {
        while !stop.load(Ordering::SeqCst) { tokio::task::yield_now().await; }
        tokio::time::sleep(Duration::from_millis(20)).await;
        done.store(true, Ordering::SeqCst);
    }));
    let stopping = state.stop_collection();
    tokio::pin!(stopping);
    assert!(tokio::time::timeout(Duration::from_millis(5), &mut stopping).await.is_err());
    assert!(state.status.read().await.enabled);
    assert_eq!(state.status.read().await.state, "STOPPING");
    stopping.await.unwrap();
    assert!(finished.load(Ordering::SeqCst));
    assert!(!state.status.read().await.enabled);
    assert!(file.with_extension("operator-stop").exists());
    assert!(!dir.0.join("as200_capture_journal.json").exists());
    let restarted = WorkerState::default();
    restarted.clone().launch(Some(dir.0.clone()), DatabaseState::default()).await;
    assert_eq!(restarted.status.read().await.state, "STOPPED");
    assert!(restarted.tasks.lock().await.is_empty());
}
use crate::plc::as200::reader::BufferSnapshot;
use std::{
    io::Write,
    process::{Child, Command, Stdio},
};

struct TestDirectory(PathBuf);
impl TestDirectory {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "golf_report_worker_test_{}_{}_{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TestDirectory {
    fn drop(&mut self) {
        if self.0.parent() == Some(std::env::temp_dir().as_path())
            && self
                .0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("golf_report_worker_test_")
        {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
fn config() -> SimulatorConfig {
    SimulatorConfig {
        enabled: true,
        source_key: "unit-test-simulator".into(),
        confirmed_session_id: Some("operator-confirmed-test-session-A".into()),
        ..Default::default()
    }
}
fn report(total: i32, sequence: i32) -> PollReport {
    let mut control = vec![0_u16; 20];
    control[0] = 1;
    control[1] = 240;
    control[2] = 2;
    control[4] = total as u16;
    control[5] = (total as u32 >> 16) as u16;
    let control = decode::control(&control, super::super::as200::WordOrder::LowWordFirst).unwrap();
    let mut raw = vec![0_u16; 240];
    raw[0] = 1;
    raw[1] = 1;
    raw[20] = 1;
    raw[4] = 0x4241; // Valid PLC ProductCode "AB" for the shared measurement fixture.
    raw[22] = sequence as u16;
    raw[23] = (sequence as u32 >> 16) as u16;
    for (header, points) in [(24, 32), (120, 128)] {
        raw[header] = 1;
        raw[header + 1] = 4;
        for p in 0..4 {
            raw[points + p * 22] = 1;
            raw[points + p * 22 + 1] = 1;
            raw[points + p * 22 + 2] = 1;
        }
    }
    let record = decode::record(&raw, 0, super::super::as200::WordOrder::LowWordFirst).unwrap();
    PollReport {
        control,
        buffers: vec![BufferSnapshot {
            buffer_index: 0,
            raw_registers: raw,
            record: Some(record),
        }],
        new_records: Vec::new(),
        diagnostics: Vec::new(),
    }
}
#[test]
fn generic_read_endpoint_never_grants_legacy_write_permissions() {
    let mut cfg = config();
    assert!(cfg.validate().is_ok());
    cfg.address = "192.168.1.10:502".into();
    assert!(cfg.validate().is_ok()); // Read-only connection has no environment selector.
    cfg.ack_enabled = true;
    cfg.ack_protocol = Some("as200".into());
    assert!(cfg.validate().is_err());
    cfg.ack_enabled = false;
    cfg.pc_heartbeat_enabled = true;
    assert!(cfg.validate().is_err());
    cfg.pc_heartbeat_enabled = false;
    cfg.address = "127.0.0.1:1503".into();
    cfg.mode = "real".into();
    assert!(cfg.validate().is_err());
    assert!(!SimulatorConfig::default().enabled);
}
#[tokio::test]
async fn durable_journal_keeps_delivery_time_and_lock_and_never_retags_old_queue() {
    let dir = TestDirectory::new();
    let path = dir.0.join("capture.json");
    let cfg = config();
    let engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    engine.capture(report(1, 1)).await.unwrap();
    engine.capture(report(1, 1)).await.unwrap();
    let original = engine.journal.lock().await.deliveries[0].input.clone();
    assert_eq!(engine.journal.lock().await.deliveries.len(), 1);
    assert!(Engine::open(cfg.clone(), path.clone(), WorkerState::default()).is_err());
    drop(engine);
    let engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    let restored = engine.journal.lock().await.deliveries[0].input.clone();
    assert_eq!(original.ingest_key, restored.ingest_key);
    assert_eq!(original.received_at, restored.received_at);
    drop(engine);
    let mut next = cfg;
    next.confirmed_session_id = Some("operator-confirmed-session-B".into());
    let engine = Engine::open(next, path, WorkerState::default()).unwrap();
    assert_eq!(
        engine.journal.lock().await.deliveries[0]
            .input
            .confirmed_session_id,
        original.confirmed_session_id
    );
}
#[tokio::test]
async fn gap_is_latched_and_reset_cannot_be_acknowledged_as_gap() {
    let dir = TestDirectory::new();
    let engine =
        Engine::open(config(), dir.0.join("capture.json"), WorkerState::default()).unwrap();
    engine.capture(report(1, 1)).await.unwrap();
    engine.capture(report(6, 6)).await.unwrap();
    assert_eq!(
        engine.journal.lock().await.pause.as_deref(),
        Some("OVERFLOW")
    );
    engine.apply_gap_ack().await.unwrap();
    assert!(engine.journal.lock().await.pause.is_none());
    assert!(!engine.journal.lock().await.warnings.is_empty());
    engine.capture(report(0, 0)).await.unwrap();
    assert_eq!(engine.journal.lock().await.pause.as_deref(), Some("RESET"));
    assert!(engine.apply_gap_ack().await.is_err());
}
#[tokio::test]
async fn conflicting_payload_never_creates_another_delivery_to_escape_error() {
    let dir = TestDirectory::new();
    let engine =
        Engine::open(config(), dir.0.join("capture.json"), WorkerState::default()).unwrap();
    engine.capture(report(1, 1)).await.unwrap();
    let mut conflict = report(1, 1);
    let record = conflict.buffers[0].record.as_mut().unwrap();
    record.raw_registers[2] = 99;
    record.stick_no = 99;
    engine.capture(conflict).await.unwrap();
    assert_eq!(engine.journal.lock().await.deliveries.len(), 1);
    assert_eq!(
        engine.journal.lock().await.pause.as_deref(),
        Some("CONFLICT")
    );
    assert!(engine.apply_gap_ack().await.is_err());
}

struct Simulator(Child);

struct MockInboxBackend {
    committed_response_lost: std::sync::Mutex<std::collections::HashSet<i64>>,
    rows: std::sync::Mutex<Vec<InboxRecord>>,
    calls: std::sync::Mutex<Vec<i64>>,
    transient: std::sync::Mutex<std::collections::HashMap<i64, usize>>,
}
impl retry::InboxBackend for MockInboxBackend {
    async fn completed(&self, source: i64, ids: Vec<i64>) -> Result<Vec<InboxRecord>, String> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .iter()
            .filter(|r| {
                r.source_id == source && ids.contains(&r.id) && r.processing_status == "PROCESSED"
            })
            .cloned()
            .collect())
    }
    async fn candidates(
        &self,
        source: i64,
        excluded: Vec<i64>,
    ) -> Result<Vec<InboxRecord>, String> {
        let mut rows = self.rows.lock().unwrap().clone();
        rows.sort_by_key(|r| (r.received_at, r.id));
        Ok(["PENDING", "FAILED"]
            .iter()
            .flat_map(|status| {
                rows.iter()
                    .filter(|r| {
                        r.source_id == source
                            && r.session_id.is_some()
                            && r.processing_status == *status
                            && !excluded.contains(&r.id)
                    })
                    .take(25)
                    .cloned()
            })
            .collect())
    }
    async fn fail(&self, id: i64, error: &str) -> Result<(), String> {
        let mut rows = self.rows.lock().unwrap();
        let row = rows.iter_mut().find(|r| r.id == id).unwrap();
        if row.measurement_id.is_none() {
            row.processing_status = "FAILED".into();
            row.error_message = Some(error.into());
        }
        Ok(())
    }
    async fn process(&self, id: i64) -> Result<InboxRecord, String> {
        self.calls.lock().unwrap().push(id);
        if let Some(left) = self.transient.lock().unwrap().get_mut(&id) {
            if *left > 0 {
                *left -= 1;
                return Err("mock PostgreSQL unavailable/commit outcome unknown".into());
            }
        }
        let mut rows = self.rows.lock().unwrap();
        let row = rows.iter_mut().find(|r| r.id == id).unwrap();
        row.processing_status = "PROCESSED".into();
        row.measurement_id = Some(id + 1000);
        row.error_message = None;
        row.processed_at = Some(Utc::now());
        if self.committed_response_lost.lock().unwrap().remove(&id) {
            return Err("mock commit succeeded but readback failed".into());
        }
        Ok(row.clone())
    }
}
fn mock_inbox(id: i64, invalid: bool) -> InboxRecord {
    let mut raw = report(id as i32, id as i32).buffers.remove(0).raw_registers;
    if invalid {
        raw[20] = 2;
    }
    InboxRecord {
        id,
        source_id: 7,
        session_id: config().confirmed_session_id,
        ingest_key: format!("mock-{id}"),
        record_seq: id as i32,
        total_count: Some(id),
        buffer_index: 0,
        raw_registers: serde_json::to_value(raw).unwrap(),
        word_order: "LOW".into(),
        received_at: DateTime::from_timestamp(1000 + id, 0).unwrap(),
        processing_status: if invalid { "FAILED" } else { "PENDING" }.into(),
        measurement_id: None,
        error_message: invalid.then(|| "old invalid row".into()),
        processed_at: None,
    }
}

#[tokio::test]
async fn historical_return_requires_unchanged_counter_and_exact_latest_record() {
    let dir = TestDirectory::new();
    let engine = Engine::open(config(), dir.0.join("capture.json"), WorkerState::default()).unwrap();
    let snapshot = report(2, 2);
    engine.capture(snapshot.clone()).await.unwrap();
    let journal = engine.journal.lock().await;
    assert!(verify_return_snapshot(&journal, &snapshot).is_ok());
    let mut changed_raw = snapshot.clone(); changed_raw.buffers[0].raw_registers[4] ^= 1;
    assert!(verify_return_snapshot(&journal, &changed_raw).is_err());
    let mut reset = snapshot.clone(); reset.control.total_count = 0;
    assert!(verify_return_snapshot(&journal, &reset).is_err());
    let mut advanced = snapshot.clone(); advanced.control.total_count = 3;
    assert!(verify_return_snapshot(&journal, &advanced).is_err());
    let mut overlap = report(3, 3);
    overlap.control.data_ready = 1;
    overlap.buffers.push(snapshot.buffers[0].clone());
    assert!(verify_return_snapshot(&journal, &overlap).is_ok());
    overlap.buffers[1].raw_registers[4] ^= 1;
    assert!(verify_return_snapshot(&journal, &overlap).is_err());
}

#[tokio::test]
async fn three_sources_keep_overlapping_sequences_and_resume_without_duplicate_mock_processing() {
    let dir = TestDirectory::new();
    let backend = mock_backend(Vec::new());
    let mut profiles = Vec::new();
    let mut next_id = 1_i64;
    for (source, count) in [(1_i64, 5_i32), (2, 1), (3, 3)] {
        let mut cfg = config();
        cfg.source_key = format!("machine-{source}");
        cfg.confirmed_session_id = Some(format!("machine-{source}-session"));
        let path = dir.0.join(format!("machine-{source}.json"));
        let engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
        engine.journal.lock().await.last_total = Some(0);
        for sequence in 1..=count {
            let snapshot = report(sequence, sequence);
            engine.capture(snapshot.clone()).await.unwrap();
            let mut row = mock_inbox(next_id, false);
            row.source_id = source; row.session_id = cfg.confirmed_session_id.clone();
            row.record_seq = sequence; row.total_count = Some(i64::from(sequence));
            row.raw_registers = serde_json::to_value(&snapshot.buffers[0].raw_registers).unwrap();
            backend.rows.lock().unwrap().push(row);
            next_id += 1;
        }
        engine.process_pending_using(&backend, source, Utc::now()).await.unwrap();
        let journal = engine.journal.lock().await;
        assert_eq!(journal.source_key, cfg.source_key);
        assert_eq!(journal.session_id, cfg.confirmed_session_id);
        assert_eq!(journal.last_total, Some(count));
        assert!(journal.ack.intent.is_none());
        assert_eq!(journal.ack.accepted, 0);
        drop(journal); drop(engine);
        profiles.push((cfg, path));
    }
    assert_eq!(backend.calls.lock().unwrap().len(), 9);
    assert_eq!(backend.rows.lock().unwrap().iter().filter(|r| r.record_seq == 1).count(), 3);
    let (cfg, path) = &profiles[0];
    let reopened = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    let previous_file = std::fs::read(path).unwrap();
    {
        let journal = reopened.journal.lock().await;
        assert!(verify_return_snapshot(&journal, &report(5, 5)).is_ok());
        assert!(verify_return_snapshot(&journal, &report(0, 0)).is_err());
        assert_eq!(journal.session_id, cfg.confirmed_session_id);
    }
    assert_eq!(std::fs::read(path).unwrap(), previous_file);
    reopened.process_pending_using(&backend, 1, Utc::now()).await.unwrap();
    assert_eq!(backend.calls.lock().unwrap().len(), 9);
    assert!(backend.rows.lock().unwrap().iter().all(|r| r.processing_status == "PROCESSED"));
}
#[test]
fn source_switch_receipt_proof_uses_database_precision_and_requires_real_processed_state() {
    let mut row = mock_inbox(1, false);
    row.processing_status = "PROCESSED".into();
    row.measurement_id = Some(1); row.processed_at = Some(Utc::now());
    let mut input = InboxInput { source_id: 0, confirmed_session_id: row.session_id.clone(),
        ingest_key: row.ingest_key.clone(), record_seq: row.record_seq, total_count: row.total_count,
        buffer_index: row.buffer_index, raw_registers: serde_json::from_value(row.raw_registers.clone()).unwrap(),
        word_order: crate::models::StoredWordOrder::Low,
        received_at: row.received_at + chrono::Duration::nanoseconds(123) };
    assert!(processed_receipt_matches(&row, &input));
    input.raw_registers[4] ^= 1;
    assert!(!processed_receipt_matches(&row, &input));
    input.raw_registers[4] ^= 1;
    row.processing_status = "PENDING".into();
    assert!(!processed_receipt_matches(&row, &input));
    row.processing_status = "PROCESSED".into(); row.measurement_id = None;
    assert!(!processed_receipt_matches(&row, &input));
}
fn mock_backend(rows: Vec<InboxRecord>) -> MockInboxBackend {
    MockInboxBackend {
        committed_response_lost: Default::default(),
        rows: std::sync::Mutex::new(rows),
        calls: Default::default(),
        transient: Default::default(),
    }
}

#[tokio::test]
async fn latest_uses_sequence_not_slot_or_receipt_time_and_ignores_archived_session() {
    let dir = TestDirectory::new();
    let engine = Engine::open(config(), dir.0.join("latest.json"), WorkerState::default()).unwrap();
    let mut snapshot = report(3, 3); // slot 0 is newer than slot 1
    let mut older = report(3, 2).buffers.remove(0);
    older.buffer_index = 1;
    older.record.as_mut().unwrap().buffer_index = 1;
    snapshot.buffers.push(older);
    engine.capture(snapshot).await.unwrap();
    let latest = engine.state.status.read().await.latest.clone().unwrap();
    assert_eq!(latest.record_seq, 3);
    assert_eq!(engine.journal.lock().await.deliveries.len(), 2);
    let timestamps: Vec<_> = engine
        .journal
        .lock()
        .await
        .deliveries
        .iter()
        .map(|d| d.input.received_at)
        .collect();
    let mut row = mock_inbox(2, false);
    row.received_at = Utc::now() + chrono::Duration::hours(1);
    engine.row_status(&row).await.unwrap();
    assert_eq!(
        engine
            .state
            .status
            .read()
            .await
            .latest
            .as_ref()
            .unwrap()
            .record_seq,
        3
    );
    row = mock_inbox(3, false);
    row.processing_status = "PROCESSED".into();
    row.measurement_id = Some(303);
    row.processed_at = Some(Utc::now());
    engine.row_status(&row).await.unwrap();
    assert_eq!(
        engine
            .state
            .status
            .read()
            .await
            .latest
            .as_ref()
            .unwrap()
            .measurement_id,
        Some(303)
    );
    row.session_id = Some("archived-session".into());
    row.record_seq = 999;
    engine.row_status(&row).await.unwrap();
    assert_eq!(
        engine
            .state
            .status
            .read()
            .await
            .latest
            .as_ref()
            .unwrap()
            .record_seq,
        3
    );
    assert_eq!(
        timestamps,
        engine
            .journal
            .lock()
            .await
            .deliveries
            .iter()
            .map(|d| d.input.received_at)
            .collect::<Vec<_>>()
    );
    assert_eq!(engine.journal.lock().await.ack.accepted, 0);
    assert!(engine.journal.lock().await.ack.intent.is_none());
}
#[tokio::test]
async fn sixty_permanent_failures_do_not_starve_new_inboxes_and_survive_restart() {
    let dir = TestDirectory::new();
    let path = dir.0.join("retry.json");
    let cfg = config();
    let engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    let mut rows: Vec<_> = (1..=60).map(|id| mock_inbox(id, true)).collect();
    rows.extend([mock_inbox(61, false), mock_inbox(62, false)]);
    let backend = mock_backend(rows);
    let now = Utc::now();
    for _ in 0..3 {
        assert!(
            engine
                .process_pending_using(&backend, 7, now)
                .await
                .is_err()
        );
    }
    assert_eq!(*backend.calls.lock().unwrap(), vec![61, 62]);
    assert_eq!(engine.journal.lock().await.retries.len(), 60);
    assert!(
        engine
            .journal
            .lock()
            .await
            .retries
            .iter()
            .all(|e| e.blocked && e.attempts == 1)
    );
    assert!(
        backend
            .rows
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.id <= 60)
            .all(|r| r.processing_status == "FAILED" && r.measurement_id.is_none())
    );
    assert_eq!(engine.journal.lock().await.ack.accepted, 0);
    drop(engine);
    let reopened = Engine::open(cfg, path, WorkerState::default()).unwrap();
    reopened
        .process_pending_using(&backend, 7, now + chrono::Duration::days(1))
        .await
        .unwrap();
    assert_eq!(*backend.calls.lock().unwrap(), vec![61, 62]);
    assert_eq!(reopened.state.status.read().await.blocked_inbox_count, 60);
}
#[tokio::test]
async fn transient_retry_backoff_restart_limit_and_explicit_retry_preserve_identity() {
    let dir = TestDirectory::new();
    let path = dir.0.join("backoff.json");
    let mut cfg = config();
    cfg.retry_max_attempts = 2;
    let mut engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    let backend = mock_backend(vec![mock_inbox(90, false)]);
    backend.transient.lock().unwrap().insert(90, 2);
    let original = backend.rows.lock().unwrap()[0].clone();
    let now = Utc::now();
    assert!(
        engine
            .process_pending_using(&backend, 7, now)
            .await
            .is_err()
    );
    engine
        .process_pending_using(&backend, 7, now + chrono::Duration::seconds(1))
        .await
        .unwrap();
    assert_eq!(backend.calls.lock().unwrap().len(), 1);
    drop(engine);
    engine = Engine::open(cfg, path, WorkerState::default()).unwrap();
    assert!(
        engine
            .process_pending_using(&backend, 7, now + chrono::Duration::seconds(2))
            .await
            .is_err()
    );
    engine
        .process_pending_using(&backend, 7, now + chrono::Duration::days(1))
        .await
        .unwrap();
    assert_eq!(backend.calls.lock().unwrap().len(), 2);
    assert!(engine.journal.lock().await.retries[0].blocked);
    assert!(engine.apply_retry_row(&original, 8).await.is_err());
    engine.apply_retry_row(&original, 7).await.unwrap();
    engine
        .process_pending_using(&backend, 7, now + chrono::Duration::days(1))
        .await
        .unwrap();
    let rows = backend.rows.lock().unwrap();
    let row = &rows[0];
    assert_eq!(row.processing_status, "PROCESSED");
    assert_eq!(row.measurement_id, Some(1090));
    assert_eq!(row.raw_registers, original.raw_registers);
    assert_eq!(row.ingest_key, original.ingest_key);
    assert_eq!(row.session_id, original.session_id);
    assert_eq!(row.record_seq, original.record_seq);
    drop(rows);
    assert!(engine.journal.lock().await.retries.is_empty());
}

#[tokio::test]
async fn committed_inbox_readback_after_restart_clears_retry_without_duplicate_processing() {
    let dir = TestDirectory::new();
    let path = dir.0.join("commit-readback.json");
    let cfg = config();
    let engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    let backend = mock_backend(vec![mock_inbox(101, false)]);
    backend.committed_response_lost.lock().unwrap().insert(101);
    let now = Utc::now();
    assert!(
        engine
            .process_pending_using(&backend, 7, now)
            .await
            .is_err()
    );
    assert_eq!(
        backend.rows.lock().unwrap()[0].processing_status,
        "PROCESSED"
    );
    drop(engine);
    let engine = Engine::open(cfg, path, WorkerState::default()).unwrap();
    engine
        .process_pending_using(&backend, 7, now + chrono::Duration::seconds(1))
        .await
        .unwrap();
    assert!(engine.journal.lock().await.retries.is_empty());
    assert_eq!(*backend.calls.lock().unwrap(), vec![101]);
    assert_eq!(
        engine
            .state
            .status
            .read()
            .await
            .latest
            .as_ref()
            .unwrap()
            .measurement_id,
        Some(1101)
    );
}

#[tokio::test]
#[ignore = "Python loopback only; heartbeat monitor/writer, no PostgreSQL or hardware"]
async fn simulator_heartbeat_worker_stall_recovery_reset_and_ack_independence() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut simulator = Simulator::start_with_options(port, false, 20, true);
    let mut cfg = config();
    cfg.address = format!("127.0.0.1:{port}");
    cfg.pc_heartbeat_enabled = true;
    cfg.plc_heartbeat_timeout_ms = Some(2000);
    let mut reader = As200Reader::new(cfg.reader());
    wait(&mut reader, 0).await;
    let dir = TestDirectory::new();
    let engine = Engine::open(cfg.clone(), dir.0.join("hb.json"), WorkerState::default()).unwrap();
    let task = tokio::spawn(engine.clone().heartbeat_loop());
    tokio::time::sleep(Duration::from_millis(2200)).await;
    assert_eq!(engine.state.status.read().await.plc_health, "RUNNING");
    let c = reader.read_control().await.unwrap();
    assert!(c.pc_heartbeat > 0);
    assert_eq!(c.pc_read_count, 0);
    assert_eq!(c.pc_read_done, 0);
    simulator.send("h hold");
    tokio::time::sleep(Duration::from_millis(3200)).await;
    assert_eq!(engine.state.status.read().await.plc_health, "STALE");
    simulator.send("h normal");
    tokio::time::sleep(Duration::from_millis(2200)).await;
    assert_eq!(engine.state.status.read().await.plc_health, "RUNNING");
    simulator.send("6");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(engine.state.status.read().await.plc_health, "DISCONNECTED");
    simulator.send("7");
    tokio::time::sleep(Duration::from_millis(2200)).await;
    assert_eq!(engine.state.status.read().await.plc_health, "RUNNING");
    simulator.send("5");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let c = wait(&mut reader, 0).await.control;
    assert_eq!(c.total_count, 0);
    assert_eq!(c.pc_read_count, 0);
    assert_eq!(c.pc_read_done, 0);
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn quarantine_survives_restart_and_keeps_good_slot_in_both_modes() {
    for mode in ["simulator", "plc"] {
        for decode_error in [false, true] {
            let dir = TestDirectory::new();
            let path = dir.0.join("diagnostics.json");
            let mut cfg = config();
            cfg.mode = mode.into();
            cfg.allow_plc_connection = mode == "plc";
            cfg.ack_enabled = true;
            cfg.ack_protocol = Some("as200".into());
            let engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
            let mut bad = report(2, 1);
            bad.buffers[0].raw_registers[20] = if decode_error { 9 } else { 2 };
            bad.buffers[0].record =
                decode::record(&bad.buffers[0].raw_registers, 0, cfg.reader().word_order).ok();
            if decode_error {
                bad.diagnostics.push(Diagnostic::InvalidRecord {
                    buffer_index: 0,
                    error: "invalid result 9".into(),
                });
            }
            let mut good = report(2, 2).buffers.remove(0);
            good.buffer_index = 1;
            good.record.as_mut().unwrap().buffer_index = 1;
            bad.buffers.push(good);
            engine.capture(bad.clone()).await.unwrap();
            engine.capture(bad.clone()).await.unwrap();
            {
                let j = engine.journal.lock().await;
                assert_eq!(j.quarantine.len(), 1);
                assert_eq!(j.quarantine[0].raw_registers, bad.buffers[0].raw_registers);
                assert_eq!(j.quarantine[0].record_seq, Some(1));
                assert_eq!(j.quarantine[0].source_key, cfg.source_key);
                assert!(!j.quarantine[0].error.is_empty());
                assert_eq!(j.deliveries.len(), 1);
                assert_eq!(j.deliveries[0].input.record_seq, 2);
                assert!(j.last_total.is_none());
            }
            engine.storage_verified.store(true, Ordering::SeqCst);
            let _ = engine.ack_step(&bad).await;
            assert!(engine.journal.lock().await.ack.intent.is_none());
            assert_eq!(engine.journal.lock().await.ack.accepted, 0);
            drop(engine);
            let restored = Engine::open(cfg, path, WorkerState::default()).unwrap();
            assert_eq!(restored.journal.lock().await.quarantine.len(), 1);
            assert_eq!(restored.journal.lock().await.deliveries.len(), 1);
        }
    }
}

#[test]
fn as200_ack_requires_release_evidence_not_echo_or_zeroed_or_overwritten_slots() {
    use crate::plc::as200::ack::{AckIntent, AckPhase, accepted_as200};
    let mut snapshot = report(1, 1);
    snapshot.control.pc_read_count = 1;
    let mut intent = AckIntent {
        target: 1,
        records: vec![(0, snapshot.buffers[0].raw_registers.clone())],
        submitted_at_ms: None,
        observed_done_high: false,
    };
    assert_eq!(intent.phase(), AckPhase::NotSent);
    assert!(!accepted_as200(&intent, &snapshot));
    intent.submitted_at_ms = Some(1);
    assert_eq!(intent.phase(), AckPhase::OutcomeUnknown);
    assert!(!accepted_as200(&intent, &snapshot)); // Count echo + Done0 không đủ.
    snapshot.buffers[0].raw_registers[1] &= !1;
    assert!(accepted_as200(&intent, &snapshot));
    let released = snapshot.clone();
    snapshot.buffers[0].raw_registers.fill(0);
    assert!(!accepted_as200(&intent, &snapshot));
    snapshot = released.clone();
    snapshot.buffers[0].raw_registers[22] = 2;
    assert!(!accepted_as200(&intent, &snapshot));
    snapshot = released.clone();
    snapshot.control.data_ready = 1;
    assert!(!accepted_as200(&intent, &snapshot));
    snapshot.control.total_count = 2;
    assert!(!accepted_as200(&intent, &snapshot)); // Total tiến không thay evidence.
    snapshot = released.clone();
    snapshot.control.pc_read_count = 2;
    assert!(!accepted_as200(&intent, &snapshot));
    snapshot = released;
    snapshot.diagnostics.push(Diagnostic::InvalidRecord {
        buffer_index: 1,
        error: "bad".into(),
    });
    assert!(!accepted_as200(&intent, &snapshot));
}

#[tokio::test]
async fn old_journal_without_quarantine_is_readable_without_retagging() {
    let dir = TestDirectory::new();
    let path = dir.0.join("old.json");
    let cfg = config();
    let engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    engine.capture(report(1, 1)).await.unwrap();
    drop(engine);
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for field in [
        "quarantine",
        "retries",
        "session_archives",
        "session_request",
    ] {
        json.as_object_mut().unwrap().remove(field);
    }
    json["version"] = serde_json::json!(1);
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    let engine = Engine::open(cfg.clone(), path, WorkerState::default()).unwrap();
    let j = engine.journal.lock().await;
    assert!(j.quarantine.is_empty());
    assert!(j.retries.is_empty());
    assert!(j.session_archives.is_empty());
    assert!(j.session_request.is_none());
    assert_eq!(j.version, 2);
    assert_eq!(
        j.deliveries[0].input.confirmed_session_id,
        cfg.confirmed_session_id
    );
}

#[tokio::test]
async fn diagnostic_limit_stops_future_reads_without_dropping_current_good_slot() {
    let dir = TestDirectory::new();
    let mut cfg = config();
    cfg.max_queue = 2;
    let engine = Engine::open(cfg, dir.0.join("limit.json"), WorkerState::default()).unwrap();
    for seq in 1..=2 {
        let mut bad = report(seq, seq);
        bad.buffers[0].raw_registers[20] = 9;
        bad.buffers[0].record = None;
        if seq == 2 {
            let mut good = report(2, 2).buffers.remove(0);
            good.buffer_index = 1;
            good.record.as_mut().unwrap().buffer_index = 1;
            bad.buffers.push(good);
        }
        engine.capture(bad).await.unwrap();
    }
    let j = engine.journal.lock().await;
    assert_eq!(j.quarantine.len(), 2);
    assert_eq!(j.pause.as_deref(), Some("DIAGNOSTIC_FULL"));
    assert_eq!(j.deliveries.len(), 1);
    assert_eq!(j.deliveries[0].input.record_seq, 2);
    drop(j);
    assert!(engine.apply_gap_ack().await.is_err());
}
impl Simulator {
    fn start_mode(port: u16, ack: bool) -> Self {
        Self::start_with_delay(port, ack, 20)
    }
    fn start_with_delay(port: u16, ack: bool, delay: u64) -> Self {
        Self::start_with_options(port, ack, delay, false)
    }
    fn start_with_options(port: u16, ack: bool, delay: u64, heartbeat: bool) -> Self {
        let folder = std::env::var_os("AS200_SIMULATOR_DIR")
            .map(PathBuf::from)
            .expect("Set AS200_SIMULATOR_DIR to the independent simulator folder");
        let python = std::env::var_os("AS200_PYTHON")
            .expect("Set AS200_PYTHON to the simulator host Python interpreter");
        let mut command = Command::new(python);
        if ack {
            command
                .arg("-B")
                .arg("-u")
                .arg("as200_simulator.py")
                .arg("--ack-handshake");
        } else {
            command.arg("-B").arg("-u").arg("as200_simulator.py");
        }
        command.arg("--ack-delay-ms").arg(delay.to_string());
        if heartbeat {
            command.arg("--pc-heartbeat");
        }
        let child = command
            .args([
                "--port",
                &port.to_string(),
                "--product-code",
                "TEST-AS200-WORKER",
            ])
            .current_dir(folder)
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self(child)
    }
    fn send(&mut self, command: &str) {
        writeln!(self.0.stdin.as_mut().unwrap(), "{command}").unwrap();
    }
}
impl Drop for Simulator {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
async fn wait(reader: &mut As200Reader, total: i32) -> PollReport {
    for _ in 0..60 {
        if let Ok(report) = reader.poll().await {
            if report.control.total_count == total {
                return report;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("Không nhận được TotalCount={total}");
}
async fn drain_ack(engine: &Engine, reader: &mut As200Reader, target: i32, pool: &PgPool) {
    for _ in 0..200 {
        if let Ok(snapshot) = reader.poll().await {
            engine.capture(snapshot.clone()).await.unwrap();
            engine.flush(pool).await.unwrap();
            engine.ack_step(&snapshot).await.unwrap();
            if engine.journal.lock().await.ack.accepted >= target {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    panic!("ACK target {target} not accepted");
}
async fn count(pool: &PgPool, source: i64) -> i64 {
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM measurements WHERE source_id=$1")
        .bind(source)
        .fetch_one(pool)
        .await
        .unwrap();
    count
}

#[tokio::test]
#[ignore = "Opt-in AS200_TEST_DATABASE=golf_report_test; giữ dữ liệu fixture đã commit dưới SourceKey/session riêng, không DELETE/DROP/TRUNCATE"]
async fn simulator_worker_postgres_end_to_end() {
    use sqlx::postgres::PgPoolOptions;
    let options = crate::database::test_support::options();
    let pool = crate::database::test_support::pool(5).await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut simulator = Simulator::start_mode(port, true);
    let dir = TestDirectory::new();
    let nonce = Utc::now().timestamp_micros();
    let mut cfg = config();
    cfg.ack_enabled = true;
    cfg.ack_protocol = Some("simulator-v1".into());
    cfg.address = format!("127.0.0.1:{port}");
    cfg.timeout_ms = 300;
    cfg.source_key = format!("as200-worker-fixture-{nonce}");
    cfg.confirmed_session_id = Some(format!("operator-test-session-A-{nonce}"));
    let path = dir.0.join("capture.json");
    let mut engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    let mut reader = As200Reader::new(cfg.reader());
    engine.capture(wait(&mut reader, 0).await).await.unwrap();
    engine.flush(&pool).await.unwrap();
    let source = SourceRepository::upsert(&pool, &cfg.source_key, None)
        .await
        .unwrap()
        .id;
    for (total, command, expected) in [(1, "1", "OK"), (2, "2", "NG"), (3, "3", "NG")] {
        simulator.send(command);
        engine
            .capture(wait(&mut reader, total).await)
            .await
            .unwrap();
        engine.flush(&pool).await.unwrap();
        let latest = engine.state.status.read().await.latest.clone().unwrap();
        assert_eq!(latest.overall_result, expected);
        assert!(latest.measurement_id.is_some());
        let saved =
            crate::services::MeasurementService::find_by_id(&pool, latest.measurement_id.unwrap())
                .await
                .unwrap();
        assert_eq!(saved.stations.len(), 2);
        assert!(saved.stations.iter().all(|s| {
            s.points.len() == 4
                && s.points
                    .iter()
                    .all(|p| p.attempts.len() == p.attempt_count as usize)
        }));
        engine.capture(reader.poll().await.unwrap()).await.unwrap();
        engine.flush(&pool).await.unwrap();
        assert_eq!(count(&pool, source).await, i64::from(total));
        drain_ack(&engine, &mut reader, total, &pool).await;
    }
    simulator.send("1");
    engine.capture(wait(&mut reader, 4).await).await.unwrap();
    let before = engine
        .journal
        .lock()
        .await
        .deliveries
        .iter()
        .find(|d| d.input.record_seq == 4)
        .unwrap()
        .input
        .clone();
    let unavailable = PgPoolOptions::new().connect_lazy_with(options);
    unavailable.close().await;
    assert!(engine.flush(&unavailable).await.is_err());
    assert_eq!(count(&pool, source).await, 3);
    engine.flush(&pool).await.unwrap();
    let after = engine
        .journal
        .lock()
        .await
        .deliveries
        .iter()
        .find(|d| d.input.record_seq == 4)
        .unwrap()
        .input
        .clone();
    assert_eq!(before.ingest_key, after.ingest_key);
    assert_eq!(before.received_at, after.received_at);
    assert_eq!(count(&pool, source).await, 4);
    drain_ack(&engine, &mut reader, 4, &pool).await;
    simulator.send("1");
    engine.capture(wait(&mut reader, 5).await).await.unwrap();
    engine.persist_receipts(&pool).await.unwrap();
    assert!(
        InboxRepository::pending_for_source(&pool, source, 20)
            .await
            .unwrap()
            .iter()
            .any(|r| r.record_seq == 5)
    );
    drop(engine);
    engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    engine.flush(&pool).await.unwrap();
    assert_eq!(count(&pool, source).await, 5);
    drain_ack(&engine, &mut reader, 5, &pool).await;
    reader = As200Reader::new(cfg.reader());
    engine.capture(wait(&mut reader, 5).await).await.unwrap();
    engine.flush(&pool).await.unwrap();
    assert_eq!(count(&pool, source).await, 5);
    simulator.send("1");
    engine.capture(wait(&mut reader, 6).await).await.unwrap();
    engine.persist_receipts(&pool).await.unwrap();
    let inbox = InboxRepository::pending_for_source(&pool, source, 20)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.record_seq == 6)
        .unwrap();
    let (a, b) = tokio::join!(
        InboxService::process(&pool, inbox.id),
        InboxService::process(&pool, inbox.id)
    );
    assert_eq!(a.unwrap(), b.unwrap());
    engine.flush(&pool).await.unwrap();
    assert_eq!(count(&pool, source).await, 6);
    let mut unknown_cfg = cfg.clone();
    unknown_cfg.source_key = format!("as200-worker-unconfirmed-fixture-{nonce}");
    unknown_cfg.confirmed_session_id = None;
    unknown_cfg.ack_enabled = false;
    let unknown = Engine::open(
        unknown_cfg.clone(),
        dir.0.join("unknown.json"),
        WorkerState::default(),
    )
    .unwrap();
    unknown.capture(reader.poll().await.unwrap()).await.unwrap();
    unknown.flush(&pool).await.unwrap();
    let unknown_source = SourceRepository::upsert(&pool, &unknown_cfg.source_key, None)
        .await
        .unwrap()
        .id;
    assert_eq!(count(&pool, unknown_source).await, 0);
    let (held,):(i64,)=sqlx::query_as("SELECT count(*) FROM plc_inbox WHERE source_id=$1 AND processing_status='SESSION_UNCONFIRMED'").bind(unknown_source).fetch_one(&pool).await.unwrap();
    assert_eq!(held, 1);
    drain_ack(&engine, &mut reader, 6, &pool).await;
    simulator.send("6");
    let mut lost = false;
    for _ in 0..30 {
        if let Err(error) = reader.poll().await {
            engine.read_error(error).await;
            lost = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(lost);
    simulator.send("7");
    simulator.send("1");
    engine.capture(wait(&mut reader, 7).await).await.unwrap();
    engine.flush(&pool).await.unwrap();
    assert_eq!(count(&pool, source).await, 7);
    drain_ack(&engine, &mut reader, 7, &pool).await;
    simulator.send("4 5");
    drain_ack(&engine, &mut reader, 12, &pool).await;
    assert!(engine.journal.lock().await.pause.is_none());
    assert_eq!(count(&pool, source).await, 12);
    tokio::time::sleep(Duration::from_millis(80)).await;
    simulator.send("5");
    let mut reset = false;
    for _ in 0..30 {
        match reader.poll().await {
            Ok(snapshot)
                if snapshot.diagnostics.iter().any(|d| {
                    matches!(
                        d,
                        Diagnostic::TrackerRejected {
                            session_unconfirmed: true,
                            ..
                        }
                    )
                }) =>
            {
                engine.capture(snapshot).await.unwrap();
                reset = true;
                break;
            }
            Err(As200Error::SessionUnconfirmed) => {
                engine.read_error(As200Error::SessionUnconfirmed).await;
                reset = true;
                break;
            }
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(reset);
    assert!(engine.apply_gap_ack().await.is_err());
    drop(engine);
    let locked = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    assert_eq!(locked.journal.lock().await.pause.as_deref(), Some("RESET"));
    drop(locked);
    let previous_session = cfg.confirmed_session_id.clone();
    cfg.confirmed_session_id = Some(format!("operator-test-session-B-{nonce}"));
    cfg.session_transition = Some(crate::plc::simulator_config::SessionTransition {
        previous_session_id: previous_session,
        previous_total_count: Some(12),
        previous_ack_count: 12,
        expected_total_count: 0,
        reason: "Opt-in fixture reset đã được xác nhận".into(),
    });
    engine = Engine::open(cfg.clone(), path, WorkerState::default()).unwrap();
    engine.flush(&pool).await.unwrap();
    reader = As200Reader::new(cfg.reader());
    let reset_snapshot = wait(&mut reader, 0).await;
    engine
        .reconcile_session(&reset_snapshot, &pool)
        .await
        .unwrap();
    simulator.send("1");
    engine.capture(wait(&mut reader, 1).await).await.unwrap();
    engine.flush(&pool).await.unwrap();
    assert_eq!(count(&pool, source).await, 13);
    let (stations,points,attempts):(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM measurement_stations s JOIN measurements m ON m.id=s.measurement_id WHERE m.source_id=$1),(SELECT count(*) FROM measurement_points p JOIN measurement_stations s ON s.id=p.station_id JOIN measurements m ON m.id=s.measurement_id WHERE m.source_id=$1),(SELECT count(*) FROM measurement_attempts a JOIN measurement_points p ON p.id=a.point_id JOIN measurement_stations s ON s.id=p.station_id JOIN measurements m ON m.id=s.measurement_id WHERE m.source_id=$1)").bind(source).fetch_one(&pool).await.unwrap();
    assert_eq!((stations, points), (26, 104));
    assert!(attempts > 0);
    println!(
        "ISOLATED FIXTURE source_key={} source_id={source}: measurements=13 stations={stations} points={points} attempts={attempts}; unknown_source={} held_inbox=1 measurements=0. No existing data deleted.",
        cfg.source_key, unknown_cfg.source_key
    );
}

#[tokio::test]
async fn ack_requires_durable_contiguous_same_session_receipts_and_unchanged_buffers() {
    let dir = TestDirectory::new();
    let mut cfg = config();
    cfg.ack_enabled = true;
    cfg.ack_protocol = Some("simulator-v1".into());
    let engine = Engine::open(cfg.clone(), dir.0.join("ack.json"), WorkerState::default()).unwrap();
    let mut snapshot = report(1, 1);
    snapshot.control.data_ready = 1;
    engine.capture(snapshot.clone()).await.unwrap();
    engine.storage_verified.store(true, Ordering::SeqCst);
    engine.ack_step(&snapshot).await.unwrap();
    assert!(engine.journal.lock().await.ack.intent.is_none()); // Chưa durable: không mở socket.
    engine.journal.lock().await.deliveries[0].inbox_id = Some(1); // Fixture giả cho unit test.
    engine.journal.lock().await.deliveries[0]
        .input
        .confirmed_session_id = Some("other-session".into());
    assert!(engine.ack_step(&snapshot).await.is_err());
    assert!(engine.journal.lock().await.ack.intent.is_none());
    engine.journal.lock().await.deliveries[0]
        .input
        .confirmed_session_id = cfg.confirmed_session_id.clone();
    engine.ack_step(&snapshot).await.unwrap();
    assert!(engine.journal.lock().await.ack.intent.is_some());
    snapshot.buffers[0].raw_registers[10] ^= 1;
    assert!(engine.ack_step(&snapshot).await.is_err()); // Không ghi sau buffer thay đổi.
    let mut after_accept = report(3, 3);
    after_accept.control.pc_read_count = 1;
    after_accept.control.data_ready = 1;
    let intent = engine.journal.lock().await.ack.intent.clone().unwrap();
    // Simulator chỉ cho Total tăng sau khi đã nhận Done, kể cả slot cũ bị thay.
    assert!(super::super::as200::ack::accepted(&intent, &after_accept));
    engine.journal.lock().await.ack.intent = None;
    engine.journal.lock().await.ack.accepted = 1;
    assert!(engine.ack_step(&snapshot).await.is_err()); // Count PLC vẫn 0, không báo thành công.
    let dir2 = TestDirectory::new();
    let engine =
        Engine::open(cfg.clone(), dir2.0.join("gap.json"), WorkerState::default()).unwrap();
    engine.storage_verified.store(true, Ordering::SeqCst);
    assert!(engine.ack_step(&report(3, 3)).await.is_err());
    let mut missing = report(2, 2);
    missing.control.data_ready = 1;
    assert!(engine.ack_step(&missing).await.is_err()); // Thiếu RecordSeq 1 dù Total chỉ tăng 2.
    cfg.mode = "plc".into();
    cfg.allow_plc_connection = true;
    assert!(
        super::super::as200::ack::AckWriter::new(
            cfg.reader(),
            "plc",
            true,
            Some("simulator-v1"),
            false
        )
        .is_err()
    );
}

// Cùng harness Simulator/PgPool hiện có; giữ fixture riêng, không xóa dữ liệu.
#[tokio::test]
#[ignore = "Opt-in AS200_TEST_DATABASE=golf_report_test; commits isolated ACK fixtures, no deletion"]
async fn simulator_ack_postgres_restart_retry_end_to_end() {
    let pool = crate::database::test_support::pool(4).await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut simulator = Simulator::start_mode(port, true);
    let dir = TestDirectory::new();
    let path = dir.0.join("ack.json");
    let mut cfg = config();
    cfg.address = format!("127.0.0.1:{port}");
    cfg.timeout_ms = 200;
    cfg.ack_enabled = true;
    cfg.ack_protocol = Some("simulator-v1".into());
    let nonce = Utc::now().timestamp_micros();
    cfg.source_key = format!("as200-ack-fixture-{nonce}");
    cfg.confirmed_session_id = Some(format!("operator-ack-fixture-session-{nonce}"));
    let mut engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    let mut reader = As200Reader::new(cfg.reader());
    wait(&mut reader, 0).await;
    let source = SourceRepository::upsert(&pool, &cfg.source_key, None)
        .await
        .unwrap()
        .id;
    for (total, command, fault) in [
        (1, "1", "normal"),
        (2, "2", "reject"),
        (3, "3", "count-error"),
        (4, "1", "accept-error"),
        (5, "2", "hold"),
    ] {
        simulator.send(&format!("9 {fault}"));
        simulator.send(command);
        let snapshot = wait(&mut reader, total).await;
        engine.capture(snapshot.clone()).await.unwrap();
        engine.ack_step(&snapshot).await.unwrap();
        assert!(engine.journal.lock().await.ack.intent.is_none()); // No ACK before DB commit.
        engine.flush(&pool).await.unwrap();
        engine.ack_step(&snapshot).await.unwrap(); // Persist intent before network.
        assert_eq!(
            engine
                .journal
                .lock()
                .await
                .ack
                .intent
                .as_ref()
                .unwrap()
                .target,
            total
        );
        if total == 1 {
            simulator.send("6");
            tokio::time::sleep(Duration::from_millis(150)).await;
            assert!(engine.ack_step(&snapshot).await.is_err()); // Offline before ACK.
            simulator.send("7");
            wait(&mut reader, total).await;
        }
        let sent = engine.ack_step(&wait(&mut reader, total).await).await;
        if matches!(fault, "reject" | "count-error" | "accept-error") {
            assert!(sent.is_err());
        } else {
            sent.unwrap();
        }
        if total == 5 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            engine
                .ack_step(&wait(&mut reader, total).await)
                .await
                .unwrap();
            assert_eq!(engine.state.status.read().await.ack_status, "FAILED");
        }
        if matches!(total, 3 | 4 | 5) {
            drop(engine); // Restart while count written / reply ambiguous / done held.
            engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
            assert!(engine.journal.lock().await.ack.intent.is_some());
        }
        if total == 4 {
            simulator.send("6");
            tokio::time::sleep(Duration::from_millis(150)).await;
            simulator.send("7"); // Offline after ACK accepted, before client confirmed.
        }
        simulator.send("9 normal");
        tokio::time::sleep(Duration::from_millis(150)).await;
        for _ in 0..5 {
            let snapshot = wait(&mut reader, total).await;
            engine.capture(snapshot.clone()).await.unwrap();
            engine.flush(&pool).await.unwrap();
            engine.ack_step(&snapshot).await.unwrap();
            if engine.journal.lock().await.ack.accepted == total {
                break;
            }
        }
        assert_eq!(engine.journal.lock().await.ack.accepted, total);
        assert_eq!(count(&pool, source).await, i64::from(total));
    }
    // Two outstanding slots, one cumulative ACK; must keep both durably first.
    simulator.send("1");
    simulator.send("3");
    let snapshot = wait(&mut reader, 7).await;
    engine.capture(snapshot.clone()).await.unwrap();
    engine.flush(&pool).await.unwrap();
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let snapshot = wait(&mut reader, 7).await;
        engine.ack_step(&snapshot).await.unwrap();
        if engine.journal.lock().await.ack.accepted == 7 {
            break;
        }
    }
    assert_eq!(engine.journal.lock().await.ack.accepted, 7);
    engine.flush(&pool).await.unwrap();
    assert_eq!(count(&pool, source).await, 7);
    let snapshot = wait(&mut reader, 7).await;
    assert_eq!(snapshot.control.pc_read_count, 7);
    assert_eq!(snapshot.control.pc_read_done, 0);
    assert_eq!(snapshot.control.data_ready, 0);
    drop(engine);
    let engine = Engine::open(cfg, path, WorkerState::default()).unwrap();
    engine.flush(&pool).await.unwrap();
    engine.ack_step(&snapshot).await.unwrap();
    assert_eq!(engine.state.status.read().await.ack_status, "ACKNOWLEDGED");
    println!(
        "ACK simulator-v1 source_id={source}: 7 committed measurements, retries/restarts without duplicates; source_key={}",
        engine.config.source_key
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "Requires Python AS200 Simulator venv; loopback mock only, no database"]
async fn as200_plc_policy_observes_done_edge_persists_it_and_never_replays_unknown_done() {
    // PLC mode + TCP mock (Python loopback), no hardware and no PostgreSQL writes.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut simulator = Simulator::start_mode(port, true);
    let dir = TestDirectory::new();
    let path = dir.0.join("plc-ack.json");
    let mut cfg = config();
    cfg.mode = "plc".into();
    cfg.allow_plc_connection = true;
    cfg.ack_enabled = true;
    cfg.ack_protocol = Some("as200".into());
    cfg.address = format!("127.0.0.1:{port}");
    cfg.timeout_ms = 200;
    let mut reader = As200Reader::new(cfg.reader());
    wait(&mut reader, 0).await;
    let mut engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    simulator.send("9 hold");
    simulator.send("1");
    let snapshot = wait(&mut reader, 1).await;
    engine.capture(snapshot.clone()).await.unwrap();
    engine.storage_verified.store(true, Ordering::SeqCst);
    engine.ack_step(&snapshot).await.unwrap();
    assert!(engine.journal.lock().await.ack.intent.is_none());
    engine.journal.lock().await.deliveries[0].inbox_id = Some(999); // Mock durable commit, no DB mutation.
    engine.storage_verified.store(true, Ordering::SeqCst);
    engine.ack_step(&snapshot).await.unwrap();
    // Restart khi mới chuẩn bị intent, chắc chắn chưa gửi Done: được tiếp tục
    // sau khi xác minh lại durability, không biến thành unknown ngay lúc mở socket.
    assert_eq!(
        engine
            .journal
            .lock()
            .await
            .ack
            .intent
            .as_ref()
            .unwrap()
            .phase(),
        crate::plc::as200::ack::AckPhase::NotSent
    );
    drop(engine);
    engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    engine.ack_step(&snapshot).await.unwrap(); // storage barrier sau restart.
    assert!(!engine.storage_verified.load(Ordering::SeqCst));
    engine.storage_verified.store(true, Ordering::SeqCst);
    engine.ack_step(&snapshot).await.unwrap();
    let high = wait(&mut reader, 1).await;
    assert_eq!(high.control.pc_read_done, 1);
    engine.storage_verified.store(true, Ordering::SeqCst);
    engine.ack_step(&high).await.unwrap();
    assert!(
        engine
            .journal
            .lock()
            .await
            .ack
            .intent
            .as_ref()
            .unwrap()
            .observed_done_high
    );
    drop(engine);
    engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    simulator.send("9 normal");
    tokio::time::sleep(Duration::from_millis(80)).await;
    let low = wait(&mut reader, 1).await;
    engine.storage_verified.store(true, Ordering::SeqCst);
    engine.ack_step(&low).await.unwrap();
    assert_eq!(engine.journal.lock().await.ack.accepted, 1);
    engine.storage_verified.store(true, Ordering::SeqCst);
    engine.ack_step(&low).await.unwrap();
    assert!(engine.journal.lock().await.ack.intent.is_none());
    // Mất response Done + bỏ lỡ cạnh: chỉ nhận nếu PLC đã xóa đúng bit valid
    // của raw đã lưu và Count/Total/DataReady đồng thuận, kể cả sau restart.
    simulator.send("9 accept-error");
    simulator.send("1");
    let snapshot = wait(&mut reader, 2).await;
    engine.capture(snapshot.clone()).await.unwrap();
    for receipt in &mut engine.journal.lock().await.deliveries {
        receipt.inbox_id = Some(999);
    }
    engine.storage_verified.store(true, Ordering::SeqCst);
    engine.ack_step(&snapshot).await.unwrap();
    engine.storage_verified.store(true, Ordering::SeqCst);
    assert!(engine.ack_step(&snapshot).await.is_err());
    tokio::time::sleep(Duration::from_millis(250)).await;
    drop(engine);
    engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    let after = wait(&mut reader, 2).await;
    assert_eq!(after.control.pc_read_done, 0);
    engine.storage_verified.store(true, Ordering::SeqCst);
    engine.ack_step(&after).await.unwrap();
    assert_eq!(engine.journal.lock().await.ack.accepted, 2);
    assert_eq!(engine.state.status.read().await.ack_status, "ACKNOWLEDGED");
    assert!(engine.journal.lock().await.ack.intent.is_none());
    // Crash sau persist, trước Done: không có evidence, không được phát lặp.
    simulator.send("9 normal");
    simulator.send("1");
    let pending = wait(&mut reader, 3).await;
    engine.capture(pending.clone()).await.unwrap();
    for d in &mut engine.journal.lock().await.deliveries {
        d.inbox_id = Some(999);
    }
    engine.storage_verified.store(true, Ordering::SeqCst);
    engine.ack_step(&pending).await.unwrap();
    {
        let mut j = engine.journal.lock().await;
        j.ack.intent.as_mut().unwrap().submitted_at_ms = Some(Utc::now().timestamp_millis() - 1000);
        engine.persist(&j).unwrap();
    }
    drop(engine);
    engine = Engine::open(cfg, path, WorkerState::default()).unwrap();
    engine.storage_verified.store(true, Ordering::SeqCst);
    engine.ack_step(&pending).await.unwrap();
    assert_eq!(
        engine.state.status.read().await.ack_status,
        "RECONCILIATION_REQUIRED"
    );
    assert_eq!(engine.journal.lock().await.ack.accepted, 2);
    let unchanged = wait(&mut reader, 3).await;
    assert_eq!(unchanged.control.pc_read_count, 2);
    assert_eq!(unchanged.control.pc_read_done, 0);
    assert!(engine.journal.lock().await.ack.intent.is_some());
    // Regression simulator-v1, no PostgreSQL writes; reset riêng fixture loopback.
    let mut simulator_cfg = engine.config.clone();
    simulator_cfg.mode = "simulator".into();
    simulator_cfg.allow_plc_connection = false;
    simulator_cfg.ack_protocol = Some("simulator-v1".into());
    simulator_cfg.confirmed_session_id = Some("simulator-regression-session".into());
    simulator.send("5");
    tokio::time::sleep(Duration::from_millis(150)).await;
    let mut simulator_reader = As200Reader::new(simulator_cfg.reader());
    wait(&mut simulator_reader, 0).await;
    let mut simulator_engine = Engine::open(
        simulator_cfg,
        dir.0.join("simulator.json"),
        WorkerState::default(),
    )
    .unwrap();
    simulator.send("1");
    let snapshot = wait(&mut simulator_reader, 1).await;
    simulator_engine.capture(snapshot.clone()).await.unwrap();
    simulator_engine.journal.lock().await.deliveries[0].inbox_id = Some(999);
    simulator_engine
        .storage_verified
        .store(true, Ordering::SeqCst);
    simulator_engine.ack_step(&snapshot).await.unwrap();
    simulator_engine
        .storage_verified
        .store(true, Ordering::SeqCst);
    simulator_engine.ack_step(&snapshot).await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    simulator_engine
        .ack_step(&wait(&mut simulator_reader, 1).await)
        .await
        .unwrap();
    assert_eq!(simulator_engine.journal.lock().await.ack.accepted, 1);
    // Count đã áp dụng nhưng trả exception: reconnect/restart vẫn tiếp tục
    // cùng target, không tạo delivery mới hoặc phát Done trước xác minh lại.
    simulator.send("9 count-error");
    simulator.send("1");
    let snapshot = wait(&mut simulator_reader, 2).await;
    simulator_engine.capture(snapshot.clone()).await.unwrap();
    for d in &mut simulator_engine.journal.lock().await.deliveries {
        d.inbox_id = Some(999);
    }
    simulator_engine
        .storage_verified
        .store(true, Ordering::SeqCst);
    simulator_engine.ack_step(&snapshot).await.unwrap();
    assert!(simulator_engine.ack_step(&snapshot).await.is_err());
    let after_count = wait(&mut simulator_reader, 2).await;
    assert_eq!(after_count.control.pc_read_count, 2);
    assert_eq!(after_count.control.pc_read_done, 0);
    let restored_cfg = simulator_engine.config.clone();
    drop(simulator_engine);
    simulator.send("9 normal");
    tokio::time::sleep(Duration::from_millis(50)).await;
    simulator_engine = Engine::open(
        restored_cfg,
        dir.0.join("simulator.json"),
        WorkerState::default(),
    )
    .unwrap();
    simulator_engine
        .storage_verified
        .store(true, Ordering::SeqCst);
    simulator_engine.ack_step(&after_count).await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    simulator_engine
        .ack_step(&wait(&mut simulator_reader, 2).await)
        .await
        .unwrap();
    assert_eq!(simulator_engine.journal.lock().await.ack.accepted, 2);
}

#[tokio::test]
async fn as200_accumulates_more_than_two_durable_records_without_simulator_freeze_assumption() {
    let dir = TestDirectory::new();
    let mut cfg = config();
    cfg.mode = "plc".into();
    cfg.allow_plc_connection = true;
    cfg.ack_enabled = true;
    cfg.ack_protocol = Some("as200".into());
    let engine = Engine::open(cfg, dir.0.join("history.json"), WorkerState::default()).unwrap();
    engine.storage_verified.store(true, Ordering::SeqCst);
    for seq in 1..=4 {
        let mut snapshot = report(seq, seq);
        snapshot.control.data_ready = 1;
        engine.capture(snapshot).await.unwrap();
        for receipt in &mut engine.journal.lock().await.deliveries {
            receipt.inbox_id = Some(1);
            receipt.terminal = true;
        }
    }
    assert_eq!(engine.journal.lock().await.deliveries.len(), 4);
    let mut snapshot = report(4, 4);
    snapshot.control.data_ready = 1;
    engine.storage_verified.store(true, Ordering::SeqCst);
    engine.ack_step(&snapshot).await.unwrap();
    let intent = engine.journal.lock().await.ack.intent.clone().unwrap();
    assert_eq!(intent.target, 4);
    let mut proof = intent.clone();
    proof.submitted_at_ms = Some(Utc::now().timestamp_millis());
    let mut newer = report(5, 5);
    newer.control.pc_read_count = 4;
    newer.control.data_ready = 1;
    assert!(!super::super::as200::ack::accepted_as200(&proof, &newer));
    proof.observed_done_high = true;
    assert!(super::super::as200::ack::accepted_as200(&proof, &newer));
}

#[tokio::test]
async fn session_change_requires_reconciliation_and_preserves_old_intent_and_receipts() {
    use crate::plc::simulator_config::SessionTransition;
    let dir = TestDirectory::new();
    let path = dir.0.join("session.json");
    let cfg = config();
    let old_session = cfg.confirmed_session_id.clone();
    let engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    engine.capture(report(3, 3)).await.unwrap();
    engine.capture(report(1, 1)).await.unwrap();
    assert_eq!(engine.journal.lock().await.pause.as_deref(), Some("RESET"));
    drop(engine);
    let engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    assert_eq!(engine.journal.lock().await.pause.as_deref(), Some("RESET"));
    drop(engine);
    let mut next = cfg;
    next.confirmed_session_id = Some("operator-confirmed-recovery-session".into());
    let engine = Engine::open(next.clone(), path.clone(), WorkerState::default()).unwrap();
    let mut empty = report(0, 0);
    empty.buffers[0].raw_registers.fill(0);
    empty.buffers[0].record = None;
    assert!(
        engine
            .session_transition_candidate(&*engine.journal.lock().await, &empty, true)
            .is_err()
    );
    assert_eq!(engine.journal.lock().await.session_id, old_session);
    assert_eq!(
        engine.journal.lock().await.pause.as_deref(),
        Some("SESSION_RECONCILIATION")
    );
    drop(engine);
    next.session_transition = Some(SessionTransition {
        previous_session_id: old_session.clone(),
        previous_total_count: Some(3),
        previous_ack_count: 0,
        expected_total_count: 0,
        reason: "Fixture reset đã được người thử xác nhận".into(),
    });
    let engine = Engine::open(next.clone(), path.clone(), WorkerState::default()).unwrap();
    let mut j = engine.journal.lock().await;
    assert!(
        engine
            .session_transition_candidate(&j, &empty, true)
            .is_err()
    ); // Raw chưa vào DB.
    j.deliveries[0].inbox_id = Some(42); // Mock durable history, không ghi PostgreSQL.
    let preserved = j.deliveries[0].input.clone();
    assert!(
        engine
            .session_transition_candidate(&j, &empty, false)
            .is_err()
    ); // Tên phiên đã được dùng.
    j.ack.intent = Some(crate::plc::as200::ack::AckIntent {
        target: 3,
        records: vec![(0, preserved.raw_registers.clone())],
        submitted_at_ms: Some(1),
        observed_done_high: false,
    });
    assert!(
        engine
            .session_transition_candidate(&j, &empty, true)
            .is_err()
    );
    assert!(j.ack.intent.is_some());
    j.ack.intent = None;
    let mut wrong = empty.clone();
    wrong.control.pc_read_count = 1;
    assert!(
        engine
            .session_transition_candidate(&j, &wrong, true)
            .is_err()
    );
    let candidate = engine
        .session_transition_candidate(&j, &empty, true)
        .unwrap();
    assert_eq!(candidate.session_archives.len(), 1);
    assert_eq!(candidate.session_archives[0].session_id, old_session);
    assert_eq!(candidate.session_archives[0].last_total, Some(3));
    assert_eq!(
        candidate.deliveries[0].input.ingest_key,
        preserved.ingest_key
    );
    assert_eq!(
        candidate.deliveries[0].input.confirmed_session_id,
        old_session
    );
    assert_eq!(
        candidate.deliveries[0].input.raw_registers,
        preserved.raw_registers
    );
    engine.persist(&candidate).unwrap();
    *j = candidate;
    drop(j);
    drop(engine);
    let reopened = Engine::open(next.clone(), path, WorkerState::default()).unwrap();
    assert_eq!(
        reopened.journal.lock().await.session_id,
        next.confirmed_session_id
    );
    assert_eq!(reopened.journal.lock().await.session_archives.len(), 1);
    assert!(reopened.journal.lock().await.pause.is_none());
}

#[tokio::test]
async fn reverting_session_request_keeps_old_ack_intent_and_restores_previous_pause() {
    let dir = TestDirectory::new();
    let path = dir.0.join("intent.json");
    let cfg = config();
    let engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    engine.capture(report(1, 1)).await.unwrap();
    {
        let mut j = engine.journal.lock().await;
        j.ack.intent = Some(crate::plc::as200::ack::AckIntent {
            target: 1,
            records: vec![(0, j.deliveries[0].input.raw_registers.clone())],
            submitted_at_ms: Some(1),
            observed_done_high: false,
        });
        engine.persist(&j).unwrap();
    }
    drop(engine);
    let mut new_cfg = cfg.clone();
    new_cfg.confirmed_session_id = Some("must-not-clear-intent".into());
    let pending = Engine::open(new_cfg, path.clone(), WorkerState::default()).unwrap();
    assert_eq!(
        pending
            .journal
            .lock()
            .await
            .ack
            .intent
            .as_ref()
            .unwrap()
            .target,
        1
    );
    pending.publish_journal().await.unwrap();
    drop(pending);
    let reverted = Engine::open(cfg.clone(), path, WorkerState::default()).unwrap();
    let j = reverted.journal.lock().await;
    assert_eq!(j.session_id, cfg.confirmed_session_id);
    assert!(j.pause.is_none());
    assert!(j.ack.intent.is_some());
    assert_eq!(j.ack.accepted, 0);
    assert_eq!(j.deliveries.len(), 1);
}

#[tokio::test]
async fn unhealthy_plc_blocks_ack_even_when_tcp_endpoint_is_available() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dir = TestDirectory::new();
    let mut cfg = config();
    cfg.ack_enabled = true;
    cfg.ack_protocol = Some("as200".into());
    cfg.address = listener.local_addr().unwrap().to_string();
    let engine = Engine::open(cfg, dir.0.join("health-gate.json"), WorkerState::default()).unwrap();
    engine.storage_verified.store(true, Ordering::SeqCst);
    for health in ["STALE", "UNCONFIGURED", "OBSERVING", "DISCONNECTED"] {
        {
            let mut status = engine.state.status.write().await;
            status.enabled = true;
            status.plc_health = health.into();
        }
        engine.ack_step(&report(1, 1)).await.unwrap();
        assert_eq!(engine.state.status.read().await.ack_status, "WAITING");
        assert!(engine.journal.lock().await.ack.intent.is_none());
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "Python loopback only; simulator process restart with ACK intent, no PostgreSQL"]
async fn simulator_process_restart_does_not_change_session_or_discard_pending_ack() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut simulator = Simulator::start_mode(port, true);
    let mut cfg = config();
    cfg.address = format!("127.0.0.1:{port}");
    cfg.ack_enabled = true;
    cfg.ack_protocol = Some("as200".into());
    let mut reader = As200Reader::new(cfg.reader());
    wait(&mut reader, 0).await;
    let dir = TestDirectory::new();
    let path = dir.0.join("restart.json");
    let engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    simulator.send("1");
    let snapshot = wait(&mut reader, 1).await;
    engine.capture(snapshot.clone()).await.unwrap();
    engine.journal.lock().await.deliveries[0].inbox_id = Some(999); // Mock durability only.
    engine.storage_verified.store(true, Ordering::SeqCst);
    engine.ack_step(&snapshot).await.unwrap();
    assert!(engine.journal.lock().await.ack.intent.is_some());
    drop(engine);
    drop(simulator);
    let _restarted = Simulator::start_mode(port, true);
    let engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    let mut reset = false;
    for _ in 0..60 {
        match reader.poll().await {
            Ok(snapshot)
                if snapshot.diagnostics.iter().any(|d| {
                    matches!(
                        d,
                        Diagnostic::TrackerRejected {
                            session_unconfirmed: true,
                            ..
                        }
                    )
                }) =>
            {
                engine.capture(snapshot).await.unwrap();
                reset = true;
                break;
            }
            Err(As200Error::SessionUnconfirmed) => {
                engine.read_error(As200Error::SessionUnconfirmed).await;
                reset = true;
                break;
            }
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(reset);
    assert_eq!(engine.journal.lock().await.pause.as_deref(), Some("RESET"));
    assert_eq!(
        engine
            .journal
            .lock()
            .await
            .ack
            .intent
            .as_ref()
            .unwrap()
            .target,
        1
    );
    assert_eq!(engine.journal.lock().await.deliveries.len(), 1);
    assert!(engine.apply_gap_ack().await.is_err());
    drop(engine);
    let old_session = cfg.confirmed_session_id.clone();
    cfg.confirmed_session_id = Some("new-after-process-reset".into());
    let blocked = Engine::open(cfg, path, WorkerState::default()).unwrap();
    assert_eq!(blocked.journal.lock().await.session_id, old_session);
    assert!(blocked.journal.lock().await.ack.intent.is_some());
    assert!(blocked.journal.lock().await.session_archives.is_empty());
}

#[tokio::test]
#[ignore = "Opt-in AS200_READONLY_DATABASE=golf_report; SELECT only + loopback Python, no DML"]
async fn postgres_retry_selection_and_session_reconciliation_read_only() {
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    let target = std::env::var("AS200_READONLY_DATABASE").expect("Explicit read-only database");
    assert_eq!(target, "golf_report");
    let app = std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("com.golf.report"));
    let settings = crate::database::config::load_config(app).unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(
            PgConnectOptions::new()
                .host(&settings.host)
                .port(settings.port)
                .database(&target)
                .username(&settings.username)
                .password(&settings.password)
                .options([("default_transaction_read_only", "on")]),
        )
        .await
        .unwrap();
    let (read_only,): (String,) = sqlx::query_as("SHOW transaction_read_only")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(read_only, "on");
    let (before,): (i64,) = sqlx::query_as("SELECT count(*) FROM plc_sources")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        InboxRepository::scheduled_for_source(&pool, -1, &[1, 2])
            .await
            .unwrap()
            .is_empty()
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let _simulator = Simulator::start_mode(port, false);
    let dir = TestDirectory::new();
    let path = dir.0.join("readonly.json");
    let mut cfg = config();
    cfg.address = format!("127.0.0.1:{port}");
    cfg.source_key = format!(
        "readonly-session-reconcile-{}",
        Utc::now().timestamp_nanos_opt().unwrap()
    );
    assert!(
        InboxRepository::processed_for_ids(&pool, -1, &[1, 2])
            .await
            .unwrap()
            .is_empty()
    );
    let old = cfg.confirmed_session_id.clone();
    let initial = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    initial.publish_journal().await.unwrap();
    drop(initial);
    cfg.confirmed_session_id = Some("new-readonly-fixture-session".into());
    cfg.session_transition = Some(crate::plc::simulator_config::SessionTransition {
        previous_session_id: old,
        previous_total_count: None,
        previous_ack_count: 0,
        expected_total_count: 0,
        reason: "Read-only integration test, empty local history".into(),
    });
    let engine = Engine::open(cfg.clone(), path, WorkerState::default()).unwrap();
    let mut reader = As200Reader::new(cfg.reader());
    let snapshot = wait(&mut reader, 0).await;
    assert!(engine.reconcile_session(&snapshot, &pool).await.is_err()); // Storage barrier.
    engine.storage_verified.store(true, Ordering::SeqCst); // Empty history, no real receipt inserted.
    engine.reconcile_session(&snapshot, &pool).await.unwrap();
    assert_eq!(
        engine.journal.lock().await.session_id,
        cfg.confirmed_session_id
    );
    assert_eq!(engine.journal.lock().await.session_archives.len(), 1);
    let (after,): (i64,) = sqlx::query_as("SELECT count(*) FROM plc_sources")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    pool.close().await;
}

#[tokio::test]
#[ignore = "Requires AS200_TEST_DATABASE=golf_report_test and Python; uses session TEMP tables, no public DML"]
async fn empty_database_startup_receive_ack_restart_and_missing_history_temp_tables() {
    use sqlx::postgres::PgPoolOptions;
    let options = crate::database::test_support::options();
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_with(options.clone())
        .await
        .unwrap();
    crate::database::test_support::verify(&pool).await;
    let (before,): (i64,) = sqlx::query_as("SELECT count(*) FROM public.measurements")
        .fetch_one(&pool)
        .await
        .unwrap();
    for table in [
        "plc_sources",
        "measurements",
        "measurement_stations",
        "measurement_points",
        "measurement_attempts",
        "plc_inbox",
    ] {
        // TEMP tables shadow application tables only on this isolated single-session pool.
        let mut create = sqlx::QueryBuilder::<sqlx::Postgres>::new("CREATE TEMP TABLE ");
        create
            .push(table)
            .push(" (LIKE public.")
            .push(table)
            .push(" INCLUDING ALL)");
        create.build().execute(&pool).await.unwrap();
        let mut defaults = sqlx::QueryBuilder::<sqlx::Postgres>::new("ALTER TABLE pg_temp.");
        defaults.push(table).push(" ALTER COLUMN id DROP DEFAULT");
        defaults.build().execute(&pool).await.unwrap();
        let mut identity = sqlx::QueryBuilder::<sqlx::Postgres>::new("ALTER TABLE pg_temp.");
        identity
            .push(table)
            .push(" ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY");
        identity.build().execute(&pool).await.unwrap();
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut simulator = Simulator::start_with_delay(port, true, 100);
    let dir = TestDirectory::new();
    let path = dir.0.join("fresh.json");
    let mut cfg = config();
    cfg.address = format!("127.0.0.1:{port}");
    cfg.ack_enabled = true;
    cfg.ack_protocol = Some("as200".into());
    let mut reader = As200Reader::new(cfg.reader());
    wait(&mut reader, 0).await;
    let mut engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    simulator.send("9 hold");
    simulator.send("1");
    let snapshot = wait(&mut reader, 1).await;
    engine.capture(snapshot.clone()).await.unwrap();
    engine.ack_step(&snapshot).await.unwrap();
    assert!(engine.journal.lock().await.ack.intent.is_none());
    let closed = PgPoolOptions::new().connect_lazy_with(options);
    closed.close().await;
    assert!(engine.flush(&closed).await.is_err());
    engine.ack_step(&snapshot).await.unwrap();
    assert!(engine.journal.lock().await.ack.intent.is_none());
    engine.flush(&pool).await.unwrap();
    assert!(engine.journal.lock().await.pause.is_none());
    drop(engine);
    engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    engine.ack_step(&snapshot).await.unwrap();
    assert!(engine.journal.lock().await.ack.intent.is_none());
    engine.flush(&pool).await.unwrap();
    engine.ack_step(&snapshot).await.unwrap();
    engine.ack_step(&snapshot).await.unwrap();
    let high = wait(&mut reader, 1).await;
    engine.ack_step(&high).await.unwrap();
    simulator.send("9 normal");
    tokio::time::sleep(Duration::from_millis(100)).await;
    engine.ack_step(&wait(&mut reader, 1).await).await.unwrap();
    assert_eq!(engine.journal.lock().await.ack.accepted, 1);
    engine.flush(&pool).await.unwrap();
    let (saved,): (i64,) = sqlx::query_as("SELECT count(*) FROM pg_temp.measurements")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(saved, 1);
    simulator.send("6");
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(reader.poll().await.is_err());
    simulator.send("7");
    wait(&mut reader, 1).await;
    simulator.send("9 hold");
    simulator.send("2");
    simulator.send("3");
    let batch = wait(&mut reader, 3).await;
    engine.capture(batch.clone()).await.unwrap();
    engine.flush(&pool).await.unwrap();
    engine.ack_step(&batch).await.unwrap();
    engine.ack_step(&batch).await.unwrap();
    let high = wait(&mut reader, 3).await;
    engine.ack_step(&high).await.unwrap();
    simulator.send("9 normal");
    tokio::time::sleep(Duration::from_millis(100)).await;
    engine.ack_step(&wait(&mut reader, 3).await).await.unwrap();
    assert_eq!(engine.journal.lock().await.ack.accepted, 3);
    engine.flush(&pool).await.unwrap();
    let (saved,): (i64,) = sqlx::query_as("SELECT count(*) FROM pg_temp.measurements")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(saved, 3);
    // Batch 5 thực sự: chờ worker lưu TEMP inbox/measurement rồi ACK qua TCP.
    simulator.send("4 5");
    drain_ack(&engine, &mut reader, 8, &pool).await;
    let (saved,): (i64,) = sqlx::query_as("SELECT count(*) FROM pg_temp.measurements")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(saved, 8);
    let (phantom,):(i64,)=sqlx::query_as("SELECT count(*) FROM measurement_attempts a JOIN measurement_points p ON p.id=a.point_id WHERE p.result='UNMEASURED'").fetch_one(&pool).await.unwrap();
    assert_eq!(phantom, 0);
    let (early_ng,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM measurement_points WHERE result='NG' AND attempt_count<>4",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(early_ng, 0);
    // Invalid NG is quarantined before inbox capture; never creates QC or advances ACK.
    simulator.send("2");
    let mut invalid = wait(&mut reader, 9).await;
    let buffer = invalid
        .buffers
        .iter_mut()
        .find(|b| b.record.as_ref().is_some_and(|r| r.record_seq == 9))
        .unwrap();
    buffer.raw_registers[151] = 2;
    buffer.raw_registers[152] = 2;
    buffer.record = Some(
        decode::record(
            &buffer.raw_registers,
            buffer.buffer_index,
            super::super::as200::WordOrder::LowWordFirst,
        )
        .unwrap(),
    );
    engine.capture(invalid.clone()).await.unwrap();
    engine.flush(&pool).await.unwrap();
    assert!(engine.ack_step(&invalid).await.is_err());
    let (failed,): (i64,) = sqlx::query_as("SELECT count(*) FROM plc_inbox WHERE record_seq=9")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(failed, 0);
    assert!(
        engine
            .journal
            .lock()
            .await
            .quarantine
            .iter()
            .any(|q| q.record_seq == Some(9) && q.raw_registers.len() == 240)
    );
    assert_eq!(engine.journal.lock().await.ack.accepted, 8);
    // Capture the unmodified PLC fixture separately to obtain a real cached receipt.
    engine.capture(reader.poll().await.unwrap()).await.unwrap();
    engine.persist_receipts(&pool).await.unwrap();
    // Journal references a deleted receipt: report specific recovery issue, do not replay or ACK.
    engine
        .journal
        .lock()
        .await
        .deliveries
        .iter_mut()
        .find(|d| d.input.record_seq == 9)
        .unwrap()
        .inbox_id = Some(999999);
    assert!(
        engine
            .flush(&pool)
            .await
            .unwrap_err()
            .contains("LOCAL_HISTORY_MISSING")
    );
    assert!(!engine.storage_verified.load(Ordering::SeqCst));
    assert_eq!(
        engine.journal.lock().await.pause.as_deref(),
        Some("LOCAL_HISTORY_MISSING")
    );
    let (after,): (i64,) = sqlx::query_as("SELECT count(*) FROM public.measurements")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    println!(
        "TEMP-only startup/DB failure/restart/ACK/missing receipt passed; public.measurements unchanged={after}"
    );
    pool.close().await; // All TEMP objects disappear with the session.
}

#[tokio::test]
async fn simulator_batch_replans_only_unsent_ack_when_new_record_arrives() {
    let dir = TestDirectory::new();
    let mut cfg = config();
    cfg.ack_enabled = true;
    cfg.ack_protocol = Some("simulator-v1".into());
    let engine = Engine::open(cfg, dir.0.join("replan.json"), WorkerState::default()).unwrap();
    let mut first = report(1, 1);
    first.control.data_ready = 1;
    engine.capture(first.clone()).await.unwrap();
    engine.storage_verified.store(true, Ordering::SeqCst);
    engine.journal.lock().await.deliveries[0].inbox_id = Some(1);
    engine.ack_step(&first).await.unwrap();
    assert_eq!(
        engine
            .journal
            .lock()
            .await
            .ack
            .intent
            .as_ref()
            .unwrap()
            .target,
        1
    );
    let mut second = report(2, 2);
    second.control.data_ready = 1;
    second.buffers[0].buffer_index = 1;
    second.buffers.insert(0, first.buffers[0].clone());
    engine.capture(second.clone()).await.unwrap();
    engine.ack_step(&second).await.unwrap();
    assert!(engine.journal.lock().await.ack.intent.is_none());
    engine.ack_step(&second).await.unwrap();
    assert!(engine.journal.lock().await.ack.intent.is_none());
    engine.journal.lock().await.deliveries[1].inbox_id = Some(2);
    engine.ack_step(&second).await.unwrap();
    assert_eq!(
        engine
            .journal
            .lock()
            .await
            .ack
            .intent
            .as_ref()
            .unwrap()
            .target,
        2
    );
}

impl retry::RetryLookup for MockInboxBackend {
    async fn find(&self, id: i64) -> Result<Option<InboxRecord>, String> {
        let mut transient = self.transient.lock().unwrap();
        if let Some(left) = transient.get_mut(&id) {
            if *left > 0 {
                *left -= 1;
                return Err("temporary lookup outage".into());
            }
        }
        Ok(self
            .rows
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.id == id)
            .cloned())
    }
}
#[tokio::test]
async fn manual_retry_missing_and_transient_requests_survive_restart_independently() {
    let dir = TestDirectory::new();
    let path = dir.0.join("manual.json");
    let engine = Engine::open(config(), path.clone(), WorkerState::default()).unwrap();
    let backend = mock_backend(vec![mock_inbox(2, false), mock_inbox(3, false)]);
    backend.transient.lock().unwrap().insert(2, 1);
    for id in [1, 2, 3] {
        engine.queue_retry(id).await.unwrap();
    }
    engine
        .apply_retry_requests_using(&backend, 7)
        .await
        .unwrap();
    let requests = engine.journal.lock().await.manual_retries.clone();
    assert_eq!(
        requests
            .iter()
            .map(|r| r.state.as_str())
            .collect::<Vec<_>>(),
        ["REJECTED", "WAITING", "APPLIED"]
    );
    assert!(backend.calls.lock().unwrap().is_empty());
    drop(engine);
    let engine = Engine::open(config(), path, WorkerState::default()).unwrap();
    engine
        .apply_retry_requests_using(&backend, 7)
        .await
        .unwrap();
    assert_eq!(
        engine.journal.lock().await.manual_retries[1].state,
        "APPLIED"
    );
    assert!(backend.calls.lock().unwrap().is_empty()); // Queueing never inserts a measurement.
    engine
        .process_pending_using(&backend, 7, Utc::now())
        .await
        .unwrap();
    let calls = backend.calls.lock().unwrap().clone();
    engine.queue_retry(2).await.unwrap();
    engine
        .apply_retry_requests_using(&backend, 7)
        .await
        .unwrap();
    engine
        .process_pending_using(&backend, 7, Utc::now())
        .await
        .unwrap();
    assert_eq!(*backend.calls.lock().unwrap(), calls); // Already PROCESSED stays idempotent.
}

struct WaitingLookup;
impl retry::RetryLookup for WaitingLookup {
    async fn find(&self, _id: i64) -> Result<Option<InboxRecord>, String> {
        std::future::pending().await
    }
}
#[tokio::test]
async fn cancelled_manual_lookup_preserves_all_unfinished_requests_on_disk() {
    let dir = TestDirectory::new();
    let path = dir.0.join("cancelled-retry.json");
    let engine = Engine::open(config(), path.clone(), WorkerState::default()).unwrap();
    engine.queue_retry(2).await.unwrap();
    engine.queue_retry(3).await.unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(20),
            engine.apply_retry_requests_using(&WaitingLookup, 7)
        )
        .await
        .is_err()
    );
    drop(engine);
    let engine = Engine::open(config(), path, WorkerState::default()).unwrap();
    let journal = engine.journal.lock().await;
    assert_eq!(journal.manual_retries.len(), 2);
    assert!(journal.manual_retries.iter().all(|r| r.state == "PENDING"));
}
#[tokio::test]
async fn metadata_and_tracker_diagnostics_are_durable_and_never_ack_receipts() {
    let dir = TestDirectory::new();
    let path = dir.0.join("identity.json");
    let engine = Engine::open(config(), path.clone(), WorkerState::default()).unwrap();
    engine.capture(report(1, 999)).await.unwrap();
    assert!(engine.journal.lock().await.deliveries.is_empty());
    assert_eq!(engine.journal.lock().await.quarantine.len(), 1);
    let mut snapshot = report(2, 1);
    let mut second = snapshot.buffers[0].clone();
    second.buffer_index = 1;
    snapshot.buffers.push(second);
    snapshot.diagnostics.push(Diagnostic::TrackerRejected {
        error: "duplicate sequence".into(),
        session_unconfirmed: false,
    });
    let expected = snapshot.control.clone();
    engine.capture(snapshot).await.unwrap();
    assert_eq!(
        engine.journal.lock().await.pause.as_deref(),
        Some("CONFLICT")
    );
    assert_eq!(engine.journal.lock().await.ack.accepted, 0);
    assert!(engine.journal.lock().await.deliveries.is_empty());
    drop(engine);
    let engine = Engine::open(config(), path, WorkerState::default()).unwrap();
    let journal = engine.journal.lock().await;
    assert_eq!(journal.quarantine.len(), 3);
    assert_eq!(journal.quarantine[1].control.as_ref(), Some(&expected));
    assert_eq!(journal.quarantine[2].raw_registers.len(), 240);
}
#[tokio::test]
async fn heartbeat_expiry_blocks_ack_while_modbus_request_is_still_waiting() {
    use super::super::as200::heartbeat::HeartbeatMonitor;
    let state = WorkerState::default();
    state.status.write().await.enabled = true;
    let now = std::time::Instant::now();
    let mut monitor = HeartbeatMonitor::new(Some(Duration::from_millis(50)), now);
    monitor.observe(0, now);
    monitor.observe(1, now);
    *state.heartbeat.lock().unwrap() = Some(monitor);
    state.status.write().await.plc_health = "RUNNING".into();
    assert!(state.fresh_health().await);
    // An actual FC03 request remains awaiting a mock response for longer than freshness.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
    });
    let mut cfg = config();
    cfg.address = address.to_string();
    cfg.timeout_ms = 1000;
    let mut reader = As200Reader::new(cfg.reader());
    let pending = tokio::spawn(async move { reader.poll().await });
    tokio::time::sleep(Duration::from_millis(90)).await;
    assert!(!pending.is_finished());
    assert!(!state.fresh_health().await);
    assert_eq!(state.status.read().await.plc_health, "STALE");
    let dir = TestDirectory::new();
    cfg.ack_enabled = true;
    cfg.ack_protocol = Some("as200".into());
    let engine = Engine::open(cfg, dir.0.join("health.json"), state).unwrap();
    engine.ack_step(&report(1, 1)).await.unwrap();
    assert_eq!(engine.state.status.read().await.ack_status, "WAITING");
    assert!(engine.journal.lock().await.ack.intent.is_none());
    pending.abort();
    server.abort();
}

struct SqlFaultBackend<'a> {
    pool: &'a PgPool,
    fail_ids: std::sync::Mutex<std::collections::HashSet<i64>>,
}
impl retry::InboxBackend for SqlFaultBackend<'_> {
    async fn completed(&self, source: i64, ids: Vec<i64>) -> Result<Vec<InboxRecord>, String> {
        InboxRepository::processed_for_ids(self.pool, source, &ids).await
    }
    async fn candidates(
        &self,
        source: i64,
        excluded: Vec<i64>,
    ) -> Result<Vec<InboxRecord>, String> {
        InboxRepository::scheduled_for_source(self.pool, source, &excluded).await
    }
    async fn fail(&self, id: i64, error: &str) -> Result<(), String> {
        InboxRepository::failed(self.pool, id, error).await
    }
    async fn process(&self, id: i64) -> Result<InboxRecord, String> {
        if self.fail_ids.lock().unwrap().remove(&id) {
            return sqlx::query("SELECT 1/0")
                .execute(self.pool)
                .await
                .map(|_| unreachable!())
                .map_err(|e| format!("injected one-attempt SQL failure: {e}"));
        }
        InboxService::process(self.pool, id).await?;
        InboxRepository::find(self.pool, id).await
    }
}
#[tokio::test]
#[ignore = "Verified golf_report_test only: SQL 60 FAILED, backoff, manual retry and journal restart"]
async fn postgres_prompt13_retry_fairness_backoff_manual_and_restart() {
    let pool = crate::database::test_support::pool(4).await;
    let mut cfg = config();
    cfg.source_key = format!(
        "prompt13-retry-{}",
        Utc::now().timestamp_nanos_opt().unwrap()
    );
    cfg.confirmed_session_id = Some(format!("session-{}", cfg.source_key));
    let source = SourceRepository::upsert(&pool, &cfg.source_key, None)
        .await
        .unwrap()
        .id;
    let dir = TestDirectory::new();
    let path = dir.0.join("prompt13-retry.json");
    let mut engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    let make = |seq: i32, bad: bool| {
        let mut row = mock_inbox(seq as i64, bad);
        row.source_id = source;
        row.session_id = cfg.confirmed_session_id.clone();
        InboxInput {
            source_id: source,
            confirmed_session_id: row.session_id,
            ingest_key: format!("sql-{seq}"),
            record_seq: seq,
            total_count: Some(seq as i64),
            buffer_index: 0,
            raw_registers: serde_json::from_value(row.raw_registers).unwrap(),
            word_order: cfg.word_order,
            received_at: Utc::now(),
        }
    };
    for seq in 1..=60 {
        let row = InboxService::receive(&pool, &make(seq, true))
            .await
            .unwrap();
        assert!(InboxService::process(&pool, row.id).await.is_err());
    }
    let fresh = InboxService::receive(&pool, &make(61, false))
        .await
        .unwrap();
    for _ in 0..3 {
        let _ = engine.process_pending(&pool, source).await;
    }
    assert_eq!(
        InboxRepository::find(&pool, fresh.id)
            .await
            .unwrap()
            .processing_status,
        "PROCESSED"
    );
    assert_eq!(
        engine
            .journal
            .lock()
            .await
            .retries
            .iter()
            .filter(|r| r.blocked)
            .count(),
        60
    );
    let delayed = InboxService::receive(&pool, &make(62, false))
        .await
        .unwrap();
    let backend = SqlFaultBackend {
        pool: &pool,
        fail_ids: std::sync::Mutex::new([delayed.id].into_iter().collect()),
    };
    let now = Utc::now();
    assert!(
        engine
            .process_pending_using(&backend, source, now)
            .await
            .is_err()
    );
    let entry = engine
        .journal
        .lock()
        .await
        .retries
        .iter()
        .find(|r| r.inbox_id == delayed.id)
        .unwrap()
        .clone();
    assert_eq!(entry.attempts, 1);
    assert!(entry.next_attempt_at > now);
    let next = InboxService::receive(&pool, &make(63, false))
        .await
        .unwrap();
    engine
        .process_pending_using(&backend, source, now)
        .await
        .unwrap();
    assert_eq!(
        InboxRepository::find(&pool, delayed.id)
            .await
            .unwrap()
            .processing_status,
        "FAILED"
    );
    assert_eq!(
        InboxRepository::find(&pool, next.id)
            .await
            .unwrap()
            .processing_status,
        "PROCESSED"
    );
    drop(engine);
    engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    engine
        .process_pending_using(&backend, source, entry.next_attempt_at)
        .await
        .unwrap();
    assert_eq!(
        InboxRepository::find(&pool, delayed.id)
            .await
            .unwrap()
            .processing_status,
        "PROCESSED"
    );
    let manual = InboxService::receive(&pool, &make(64, false))
        .await
        .unwrap();
    backend.fail_ids.lock().unwrap().insert(manual.id);
    assert!(
        engine
            .process_pending_using(&backend, source, Utc::now())
            .await
            .is_err()
    );
    engine.queue_retry(i64::MAX).await.unwrap();
    engine.queue_retry(manual.id).await.unwrap();
    drop(engine);
    engine = Engine::open(cfg, path, WorkerState::default()).unwrap();
    engine.apply_retry_requests(&pool, source).await.unwrap();
    let requests = engine.journal.lock().await.manual_retries.clone();
    assert_eq!(requests[0].state, "REJECTED");
    assert_eq!(requests[1].state, "APPLIED");
    engine.process_pending(&pool, source).await.unwrap();
    engine.process_pending(&pool, source).await.unwrap();
    let counts:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM plc_inbox WHERE source_id=$1 AND processing_status='FAILED'),(SELECT count(*) FROM plc_inbox WHERE source_id=$1 AND processing_status='PROCESSED'),(SELECT count(*) FROM measurements WHERE source_id=$1)").bind(source).fetch_one(&pool).await.unwrap();
    assert_eq!(counts, (60, 4, 4));
    assert_eq!(engine.journal.lock().await.ack.accepted, 0);
    assert!(engine.journal.lock().await.ack.intent.is_none());
    println!(
        "PASS SQL retry source={source}: FAILED=60 PROCESSED=4 measurement=4; new rows progress, backoff/restart/manual retry verified; no ACK writer enabled"
    );
}
