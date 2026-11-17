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
        "publishedRecordSeq", "manualRetryResults", "retryDiagnostics", "volatileCount",
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
impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "golf-regression-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
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
        let (memory, events, fault) = (words.clone(), writes.clone(), drop_reply.clone());
        let task = tokio::spawn(async move {
            let mut clients = tokio::task::JoinSet::new();
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (memory, events, fault) = (memory.clone(), events.clone(), fault.clone());
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
async fn database_unavailable_prevents_snapshot_and_any_ack_write() {
    let plc = Plc::new().await;
    let dir = TestDirectory::new();
    let mut cfg = config();
    cfg.address = plc.address.clone();
    let e = engine(cfg, &dir, 1);
    assert!(
        e.poll_and_ack_published(&closed_pool().await)
            .await
            .is_err()
    );
    assert!(plc.writes.lock().await.is_empty());
    assert!(e.state.status.read().await.control_snapshot.is_none());
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
    e.read_snapshot().await.unwrap();
    assert!(e.state.fresh_health().await);
    assert!(plc.writes.lock().await.is_empty());
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
        rows.iter()
            .all(|r| r.record_seq == 0 && r.measurement_id.is_none())
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
            .decode_error
            .is_some()
    );
    let e = restart(e, &dir, &pool).await;
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
