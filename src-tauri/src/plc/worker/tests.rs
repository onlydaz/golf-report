use super::*;
use crate::plc::as200::reader::BufferSnapshot;

#[tokio::test]
async fn stop_waits_for_the_collection_task_and_keeps_the_operator_marker() {
    let dir = TestDirectory::new();
    let file = dir.0.join("as200_config.toml");
    std::fs::write(&file, "enabled = true\n").unwrap();
    let state = WorkerState::default();
    *state.settings_path.lock().await = Some(file.clone());
    state.status.write().await.enabled = true;
    let finished = Arc::new(AtomicBool::new(false));
    let done = finished.clone();
    let stop = state.stopping.clone();
    state
        .tasks
        .lock()
        .await
        .push(tauri::async_runtime::spawn(async move {
            while !stop.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            done.store(true, Ordering::SeqCst);
        }));
    let stopping = state.stop_collection();
    tokio::pin!(stopping);
    assert!(
        tokio::time::timeout(Duration::from_millis(5), &mut stopping)
            .await
            .is_err()
    );
    assert_eq!(state.status.read().await.state, "STOPPING");
    stopping.await.unwrap();
    assert!(finished.load(Ordering::SeqCst));
    assert!(!state.status.read().await.enabled);
    assert!(file.with_extension("operator-stop").exists());
    assert!(!dir.0.join("as200_capture_journal.json").exists());
    let restarted = WorkerState::default();
    restarted
        .clone()
        .launch(Some(dir.0.clone()), DatabaseState::default())
        .await;
    assert_eq!(restarted.status.read().await.state, "STOPPED");
    assert!(restarted.tasks.lock().await.is_empty());
}

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
        mode: "modbus".into(),
        technical_setup_confirmed: true,
        ack_enabled: true,
        ack_protocol: Some("as200".into()),
        allow_plc_connection: true,
        plc_heartbeat_timeout_ms: Some(5000),
        enabled: true,
        source_key: "unit-test-simulator".into(),
        confirmed_session_id: Some("operator-confirmed-test-session-A".into()),
        ..Default::default()
    }
}
fn published_snapshot(total: i32, sequence: i32) -> PublishedRecordSnapshot {
    let mut control = vec![0_u16; 20];
    control[0] = 1;
    control[1] = 240;
    control[2] = 2;
    control[11] = 1;
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
    PublishedRecordSnapshot {
        control,
        buffer: Some(BufferSnapshot {
            buffer_index: 0,
            raw_registers: raw,
            record: Some(record),
        }),
        diagnostics: Vec::new(),
    }
}
#[tokio::test]
#[ignore = "Manual fixture only: localhost Simulator, local golf_report_test, preserved v4 runtime"]
async fn published_record_manual_harness() {
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::str::FromStr;

    let url = std::env::var("PUBLISHED_TEST_DATABASE_URL")
        .expect("Set PUBLISHED_TEST_DATABASE_URL for local golf_report_test only");
    let options = PgConnectOptions::from_str(&url).expect("Invalid test database URL");
    assert_eq!(options.get_database(), Some("golf_report_test"));
    assert!(matches!(
        options.get_host(),
        "localhost" | "127.0.0.1" | "::1"
    ));
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect_with(options.clone())
        .await
        .expect("Cannot connect to isolated test DB");
    crate::database::test_support::verify(&pool).await;
    // Closed independent pool injects storage failure without stopping PostgreSQL
    // or changing permissions/data. Never close a clone of the live pool.
    let unavailable = PgPoolOptions::new().connect_lazy_with(options);
    unavailable.close().await;

    let runtime = PathBuf::from(
        std::env::var_os("PUBLISHED_TEST_RUNTIME")
            .expect("Set a dedicated absolute PUBLISHED_TEST_RUNTIME; keep it for recovery"),
    );
    assert!(runtime.is_absolute());
    std::fs::create_dir_all(&runtime).unwrap();
    let mut cfg = config();
    let port: u16 = std::env::var("PUBLISHED_TEST_PORT")
        .unwrap_or_else(|_| "1503".into())
        .parse()
        .expect("Invalid loopback simulator port");
    assert_ne!(port, 0);
    cfg.address = format!("127.0.0.1:{port}");
    cfg.source_key =
        std::env::var("PUBLISHED_TEST_SOURCE").expect("Set a dedicated fixture SourceKey");
    assert!(cfg.source_key.starts_with("published-fixture-") && cfg.source_key.len() > 18);
    cfg.confirmed_session_id = Some(
        std::env::var("PUBLISHED_TEST_SESSION")
            .expect("Set fixture session; preserve it across restart"),
    );
    let path = runtime.join("published-v4.json");
    let engine = Engine::open(cfg, path, WorkerState::default()).unwrap();
    engine.verify_open(&pool).await.unwrap();
    let seconds: u64 = std::env::var("PUBLISHED_TEST_SECONDS")
        .unwrap_or_else(|_| "180".into())
        .parse()
        .expect("Invalid duration");
    assert!((1..=3600).contains(&seconds));
    println!(
        "PUBLISHED fixture only; runtime={}; no automatic cleanup",
        runtime.display()
    );
    let until = std::time::Instant::now() + Duration::from_secs(seconds);
    let mut last = String::new();
    while std::time::Instant::now() < until {
        let database = if runtime.join("database-offline").exists() {
            &unavailable
        } else {
            &pool
        };
        let result = engine.poll_and_ack_published(database).await;
        let journal = engine.journal.lock().await;
        let progress = journal.published_ack.as_ref().unwrap();
        let observation = format!(
            "accepted={} phase={:?} observed_done_high={} pause={:?} queued={} result={:?}",
            progress.accepted,
            progress.intent.as_ref().map(|i| i.phase),
            progress
                .intent
                .as_ref()
                .is_some_and(|i| i.observed_done_high),
            journal.pause,
            journal.deliveries.len(),
            result
        );
        drop(journal);
        if observation != last {
            println!("{observation}");
            last = observation;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    // Ending this harness is not an assertion that ACK succeeded. Inspect the
    // observations and preserved v4 journal against the scenario expectations.
    pool.close().await;
}
#[tokio::test]
async fn published_ack_uncertain_phases_survive_restart_without_becoming_prepared() {
    for phase in [
        PublishedAckPhase::Prepared,
        PublishedAckPhase::CountWriteUncertain,
        PublishedAckPhase::CountVerified,
        PublishedAckPhase::DoneWriteUncertain,
    ] {
        let dir = TestDirectory::new();
        let path = dir.0.join("v4.json");
        let cfg = config();
        let engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
        engine
            .capture_published_snapshot(published_snapshot(1, 1))
            .await
            .unwrap();
        {
            let mut journal = engine.journal.lock().await;
            journal.deliveries[0].inbox_id = Some(42); // Serialization fixture; no database or write is invoked.
            let raw = journal.deliveries[0].input.raw_registers.clone();
            journal.published_ack.as_mut().unwrap().intent = Some(PublishedAckIntent {
                target: 1,
                inbox_id: 42,
                raw_registers: raw,
                phase,
                observed_done_high: false,
            });
            engine.persist(&journal).unwrap();
        }
        drop(engine);
        let reopened = Engine::open(cfg, path, WorkerState::default()).unwrap();
        assert_eq!(
            reopened
                .journal
                .lock()
                .await
                .published_ack
                .as_ref()
                .unwrap()
                .intent
                .as_ref()
                .unwrap()
                .phase,
            phase
        );
    }
}

#[tokio::test]
async fn published_ack_historical_anchor_allows_next_sequence_without_starting_at_one() {
    let dir = TestDirectory::new();
    let engine = Engine::open(config(), dir.0.join("v4.json"), WorkerState::default()).unwrap();
    let mut journal = engine.journal.lock().await;
    let mut old = published_snapshot(10, 10);
    let record = old.buffer.take().unwrap().raw_registers;
    let input = InboxInput {
        source_id: 0,
        confirmed_session_id: journal.session_id.clone(),
        ingest_key: "anchor".into(),
        record_seq: 10,
        total_count: Some(10),
        buffer_index: 0,
        raw_registers: record.clone(),
        word_order: engine.config.word_order,
        received_at: Utc::now(),
    };
    let progress = journal.published_ack.as_mut().unwrap();
    progress.accepted = 10;
    progress.confirmed_receipt = Some(Delivery {
        input,
        inbox_id: Some(42),
        terminal: true,
    });
    progress.intent = Some(PublishedAckIntent {
        target: 10,
        inbox_id: 42,
        raw_registers: record,
        phase: PublishedAckPhase::AckConfirmed,
        observed_done_high: true,
    });
    journal.last_total = Some(10);
    let mut snapshot = published_snapshot(12, 11);
    snapshot.control.pc_read_count = 10;
    let (input, _) = engine
        .published_delivery(&journal, &snapshot)
        .unwrap()
        .unwrap();
    assert_eq!(input.record_seq, 11);
    assert_eq!(input.total_count, Some(12)); // Observation is not ACK target; write verification blocks this case.
    let mut old_again = published_snapshot(12, 10);
    old_again.control.pc_read_count = 10;
    assert!(engine.published_delivery(&journal, &old_again).is_err());
}

#[tokio::test]
async fn published_ack_compacts_only_confirmed_receipt_and_keeps_anchor_and_pending_data() {
    let dir = TestDirectory::new();
    let path = dir.0.join("v4.json");
    let mut cfg = config();
    cfg.max_queue = 2;
    let engine = Engine::open(cfg.clone(), path.clone(), WorkerState::default()).unwrap();
    engine
        .capture_published_snapshot(published_snapshot(2, 1))
        .await
        .unwrap();
    engine
        .capture_published_snapshot(published_snapshot(2, 2))
        .await
        .unwrap();
    let mut saved = engine.journal.lock().await.clone();
    saved.deliveries[0].inbox_id = Some(42);
    saved.deliveries[0].terminal = true;
    let delivery = saved.deliveries[0].clone(); // State-only fixture; DB verification remains in confirm_published_ack.
    let mut intent = PublishedAckIntent {
        target: 1,
        inbox_id: 42,
        raw_registers: delivery.input.raw_registers.clone(),
        phase: PublishedAckPhase::DoneWriteUncertain,
        observed_done_high: false,
    };
    let mut after = published_snapshot(2, 2);
    after.control.pc_read_count = 1;
    assert!(
        engine
            .published_confirmation_candidate(&saved, &intent, &delivery, &after)
            .is_err()
    );
    intent.observed_done_high = true;
    saved.published_ack.as_mut().unwrap().intent = Some(intent.clone());
    let next = engine
        .published_confirmation_candidate(&saved, &intent, &delivery, &after)
        .unwrap();
    assert_eq!(next.deliveries.len(), 1);
    assert_eq!(next.deliveries[0].input.record_seq, 2);
    assert!(next.deliveries[0].inbox_id.is_none()); // Unprocessed data is never compacted.
    let progress = next.published_ack.as_ref().unwrap();
    assert_eq!(progress.accepted, 1);
    assert_eq!(
        progress.confirmed_receipt.as_ref().unwrap().inbox_id,
        Some(42)
    );
    assert_eq!(
        progress.intent.as_ref().unwrap().phase,
        PublishedAckPhase::AckConfirmed
    );
    engine.persist(&next).unwrap();
    drop(engine);
    let reopened = Engine::open(cfg, path, WorkerState::default()).unwrap();
    assert_eq!(reopened.journal.lock().await.deliveries.len(), 1);
}

#[tokio::test]
async fn published_ack_phase_persist_failure_blocks_further_writes() {
    let dir = TestDirectory::new();
    let path = dir.0.join("v4.json");
    let engine = Engine::open(config(), path.clone(), WorkerState::default()).unwrap();
    engine
        .capture_published_snapshot(published_snapshot(1, 1))
        .await
        .unwrap();
    let mut intent = {
        let mut journal = engine.journal.lock().await;
        journal.deliveries[0].inbox_id = Some(42);
        let intent = PublishedAckIntent {
            target: 1,
            inbox_id: 42,
            raw_registers: journal.deliveries[0].input.raw_registers.clone(),
            phase: PublishedAckPhase::Prepared,
            observed_done_high: false,
        };
        journal.published_ack.as_mut().unwrap().intent = Some(intent.clone());
        engine.persist(&journal).unwrap();
        intent
    };
    std::fs::create_dir(path.with_extension("tmp")).unwrap();
    intent.phase = PublishedAckPhase::CountWriteUncertain;
    assert!(engine.update_published_intent(intent).await.is_err());
    let journal = engine.journal.lock().await;
    assert_eq!(journal.pause.as_deref(), Some("JOURNAL_WRITE_FAILED"));
    assert_eq!(
        journal
            .published_ack
            .as_ref()
            .unwrap()
            .intent
            .as_ref()
            .unwrap()
            .phase,
        PublishedAckPhase::Prepared
    );
}

#[test]
fn source_switch_receipt_proof_uses_database_precision_and_requires_real_processed_state() {
    let mut row = mock_inbox(1, false);
    row.processing_status = "PROCESSED".into();
    row.measurement_id = Some(1);
    row.processed_at = Some(Utc::now());
    let mut input = InboxInput {
        source_id: 0,
        confirmed_session_id: row.session_id.clone(),
        ingest_key: row.ingest_key.clone(),
        record_seq: row.record_seq,
        total_count: row.total_count,
        buffer_index: row.buffer_index,
        raw_registers: serde_json::from_value(row.raw_registers.clone()).unwrap(),
        word_order: crate::models::StoredWordOrder::Low,
        received_at: row.received_at + chrono::Duration::nanoseconds(123),
    };
    assert!(processed_receipt_matches(&row, &input));
    input.raw_registers[4] ^= 1;
    assert!(!processed_receipt_matches(&row, &input));
    input.raw_registers[4] ^= 1;
    row.processing_status = "PENDING".into();
    assert!(!processed_receipt_matches(&row, &input));
    row.processing_status = "PROCESSED".into();
    row.measurement_id = None;
    assert!(!processed_receipt_matches(&row, &input));
}

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
    let mut raw = published_snapshot(id as i32, id as i32)
        .buffer
        .unwrap()
        .raw_registers;
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

fn mock_backend(rows: Vec<InboxRecord>) -> MockInboxBackend {
    MockInboxBackend {
        committed_response_lost: Default::default(),
        rows: std::sync::Mutex::new(rows),
        calls: Default::default(),
        transient: Default::default(),
    }
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
    assert_eq!(
        engine
            .journal
            .lock()
            .await
            .published_ack
            .as_ref()
            .unwrap()
            .accepted,
        0
    );
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

#[test]
fn old_journals_with_or_without_intent_are_rejected_without_rewriting() {
    let dir = TestDirectory::new();
    for version in [1, 2, 3] {
        let path = dir.0.join(format!("v{version}.json"));
        let original = serde_json::json!({ "version": version, "source_key": config().source_key,
            "ack": { "accepted": 2, "intent": { "target": 3 } } })
        .to_string();
        std::fs::write(&path, &original).unwrap();
        assert!(Engine::open(config(), path.clone(), WorkerState::default()).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    }
}

#[tokio::test]
async fn published_capture_deduplicates_and_blocks_changed_raw_and_gaps() {
    let dir = TestDirectory::new();
    let path = dir.0.join("capture.json");
    let engine = Engine::open(config(), path.clone(), WorkerState::default()).unwrap();
    engine
        .capture_published_snapshot(published_snapshot(1, 1))
        .await
        .unwrap();
    let key = engine.journal.lock().await.deliveries[0]
        .input
        .ingest_key
        .clone();
    engine
        .capture_published_snapshot(published_snapshot(1, 1))
        .await
        .unwrap();
    assert_eq!(engine.journal.lock().await.deliveries.len(), 1);
    let mut changed = published_snapshot(1, 1);
    let buffer = changed.buffer.as_mut().unwrap();
    buffer.raw_registers[4] ^= 1;
    buffer.record =
        Some(decode::record(&buffer.raw_registers, 0, config().reader().word_order).unwrap());
    assert!(engine.capture_published_snapshot(changed).await.is_err());
    drop(engine);
    let reopened = Engine::open(config(), path, WorkerState::default()).unwrap();
    assert_eq!(
        reopened.journal.lock().await.deliveries[0].input.ingest_key,
        key
    );
    assert!(reopened.journal.lock().await.pause.is_some());
    let other = Engine::open(config(), dir.0.join("gap.json"), WorkerState::default()).unwrap();
    assert!(
        other
            .capture_published_snapshot(published_snapshot(2, 2))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn heartbeat_observation_is_read_only_and_stale_health_blocks_writes() {
    let state = WorkerState::default();
    state.status.write().await.enabled = true;
    let now = std::time::Instant::now();
    let mut monitor =
        super::super::as200::heartbeat::HeartbeatMonitor::new(Some(Duration::from_secs(5)), now);
    monitor.observe(1, now);
    monitor.observe(2, now + Duration::from_secs(1));
    *state.heartbeat.lock().unwrap() = Some(monitor);
    state.status.write().await.plc_health = "RUNNING".into();
    assert!(state.fresh_health().await);
    state.status.write().await.plc_health = "DISCONNECTED".into();
    assert!(!state.fresh_health().await);
}

#[tokio::test]
async fn closed_reset_stays_with_its_source_and_never_releases_pending_data_or_intent() {
    let dir = TestDirectory::new();
    let engine = Engine::open(config(), dir.0.join("reset.json"), WorkerState::default()).unwrap();
    let mut journal = engine.journal.lock().await.clone();
    let raw = published_snapshot(2, 2).buffer.unwrap().raw_registers;
    let receipt = Delivery {
        input: InboxInput {
            source_id: 0,
            confirmed_session_id: journal.session_id.clone(),
            ingest_key: "closed-anchor".into(),
            record_seq: 2,
            total_count: Some(2),
            buffer_index: 0,
            raw_registers: raw.clone(),
            word_order: config().word_order,
            received_at: Utc::now(),
        },
        inbox_id: Some(42),
        terminal: true,
    };
    let progress = journal.published_ack.as_mut().unwrap();
    progress.accepted = 2;
    progress.confirmed_receipt = Some(receipt);
    progress.intent = Some(PublishedAckIntent {
        target: 2,
        inbox_id: 42,
        raw_registers: raw,
        phase: PublishedAckPhase::AckConfirmed,
        observed_done_high: true,
    });
    journal.last_total = Some(2);
    journal.pause = Some("RESET".into());
    journal.validate(&config()).unwrap();
    let before = serde_json::to_value(&journal).unwrap();
    assert!(journal.closed(true).is_ok());
    assert!(journal.closed(false).is_err());
    assert_eq!(before, serde_json::to_value(&journal).unwrap());
    journal.last_total = Some(3);
    assert!(journal.closed(true).is_err());
    journal.last_total = Some(2);
    journal
        .published_ack
        .as_mut()
        .unwrap()
        .intent
        .as_mut()
        .unwrap()
        .phase = PublishedAckPhase::DoneWriteUncertain;
    assert!(journal.closed(true).is_err());
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
    assert_eq!(
        engine
            .journal
            .lock()
            .await
            .published_ack
            .as_ref()
            .unwrap()
            .accepted,
        0
    );
    assert!(
        engine
            .journal
            .lock()
            .await
            .published_ack
            .as_ref()
            .unwrap()
            .intent
            .is_none()
    );
    println!(
        "PASS SQL retry source={source}: FAILED=60 PROCESSED=4 measurement=4; new rows progress, backoff/restart/manual retry verified; no ACK write invoked"
    );
}
