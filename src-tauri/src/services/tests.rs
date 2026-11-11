use super::{
    InboxService, MeasurementService,
    inbox_service::{draft_from_inbox, real32_numeric},
};
use crate::{enums::MeasurementResult, models::*, repositories::*};
use chrono::Utc;

fn registers(seq: i32) -> Vec<u16> {
    let mut raw = vec![0; 240];
    raw[0] = 1;
    raw[1] = 1;
    raw[2] = 42;
    raw[4] = 0x4241;
    raw[20] = 2;
    raw[22] = seq as u16;
    raw[23] = (seq as u32 >> 16) as u16;
    for (header, points, load) in [(24, 32, 125.5_f32), (120, 128, 165.25_f32)] {
        raw[header] = 1;
        raw[header + 1] = 4;
        let bits = load.to_bits();
        raw[header + 4] = bits as u16;
        raw[header + 5] = (bits >> 16) as u16;
        for point in 0..4 {
            let base = points + point * 22;
            raw[base] = 1;
            raw[base + 1] = 4;
            raw[base + 2] = 4;
            for attempt in 0..4 {
                let bits = (1.25_f32 + attempt as f32).to_bits();
                raw[base + 6 + attempt * 4] = bits as u16;
                raw[base + 7 + attempt * 4] = (bits >> 16) as u16;
                let bits = (-0.5_f32 - attempt as f32).to_bits();
                raw[base + 8 + attempt * 4] = bits as u16;
                raw[base + 9 + attempt * 4] = (bits >> 16) as u16;
            }
        }
    }
    // NG ở điểm cuối TIP sau đủ bốn lượt; các điểm trước đó OK.
    raw[120] = 2;
    raw[122] = 4;
    raw[194] = 2;
    raw
}
fn input(source: i64, seq: i32, session: Option<&str>, key: &str) -> InboxInput {
    InboxInput {
        source_id: source,
        confirmed_session_id: session.map(str::to_owned),
        ingest_key: key.into(),
        record_seq: seq,
        total_count: Some(i64::from(seq)),
        buffer_index: 0,
        raw_registers: registers(seq),
        word_order: StoredWordOrder::Low,
        received_at: Utc::now(),
    }
}
fn row(input: InboxInput) -> InboxRecord {
    InboxRecord {
        id: 1,
        source_id: input.source_id,
        session_id: input.confirmed_session_id,
        ingest_key: input.ingest_key,
        record_seq: input.record_seq,
        total_count: input.total_count,
        buffer_index: input.buffer_index,
        raw_registers: serde_json::to_value(input.raw_registers).unwrap(),
        word_order: input.word_order.as_str().into(),
        received_at: input.received_at,
        processing_status: "PENDING".into(),
        measurement_id: None,
        error_message: None,
        processed_at: None,
    }
}
#[test]
fn conversion_preserves_results_loads_attempts_and_received_time() {
    let inbox = row(input(
        1,
        10,
        Some("externally-confirmed-test-session"),
        "receipt",
    ));
    let draft = draft_from_inbox(&inbox).unwrap();
    assert_eq!(draft.measured_at, inbox.received_at);
    assert_eq!(draft.overall_result, MeasurementResult::Ng);
    assert_eq!(draft.club_number, 42);
    assert_eq!(draft.product_code, "AB");
    assert_eq!(draft.stations[0].station_type, StationType::Tip);
    assert_eq!(draft.stations[0].load.to_string(), "165.25");
    assert_eq!(draft.stations[1].load.to_string(), "125.5");
    assert_eq!(draft.stations[0].points[0].attempt_count, 4);
    assert_eq!(draft.stations[0].points[0].decide_attempt, Some(4));
    assert_eq!(
        draft.stations[0].points[0].attempts[3]
            .bend
            .as_ref()
            .unwrap()
            .to_string(),
        "-3.5"
    );
    assert_eq!(
        draft.stations[0].points[0].attempts[0]
            .push_dist
            .as_ref()
            .unwrap()
            .to_string(),
        "1.25"
    );
}
#[test]
fn unmeasured_is_preserved_and_unused_attempts_are_not_inserted() {
    let mut inbox = row(input(1, 1, Some("confirmed"), "receipt"));
    let mut raw = vec![0_u16; 240];
    raw[0] = 1;
    raw[1] = 1;
    raw[22] = 1;
    raw[4] = 0x4241;
    inbox.raw_registers = serde_json::to_value(raw).unwrap();
    let draft = draft_from_inbox(&inbox).unwrap();
    assert_eq!(draft.overall_result, MeasurementResult::Unmeasured);
    for station in draft.stations {
        for point in station.points {
            assert_eq!(point.result, MeasurementResult::Unmeasured);
            assert_eq!(point.decide_attempt, None);
            assert!(point.attempts.is_empty());
        }
    }
}
#[test]
fn missing_or_control_character_product_code_never_becomes_a_draft() {
    for value in [0_u16, 9_u16] {
        let mut input = input(1, 1, Some("confirmed"), "bad-product");
        input.raw_registers[4] = value;
        let inbox = row(input);
        assert!(draft_from_inbox(&inbox).unwrap_err().contains("ProductCode"));
    }
}
#[test]
fn rejects_unconfirmed_session_nonfinite_and_inconsistent_structure() {
    assert!(draft_from_inbox(&row(input(1, 1, None, "receipt"))).is_err());
    assert!(real32_numeric(f32::NAN).is_err());
    assert!(real32_numeric(f32::INFINITY).is_err());
    assert_eq!(real32_numeric(0.1).unwrap().to_string(), "0.1");
    let mut draft = draft_from_inbox(&row(input(1, 1, Some("confirmed"), "receipt"))).unwrap();
    draft.stations[0].points[0].attempt_count = 3;
    assert!(MeasurementService::validate(&draft).is_err());
    draft.stations[0].points[0].attempt_count = 4;
    draft.stations[0].points[0].decide_attempt = Some(5);
    assert!(MeasurementService::validate(&draft).is_err());
}

#[test]
fn inbox_conversion_uses_common_business_validation_without_rewriting_raw() {
    let original = row(input(1, 1, Some("confirmed"), "invalid-business"));
    for offset in [20, 121, 122, 195, 196] {
        let mut invalid = original.clone();
        let mut raw: Vec<u16> = serde_json::from_value(invalid.raw_registers.clone()).unwrap();
        raw[offset] = match offset {
            20 => 1,
            121 => 3,
            122 => 2,
            _ => 2,
        };
        invalid.raw_registers = serde_json::to_value(raw).unwrap();
        let preserved = invalid.raw_registers.clone();
        assert!(draft_from_inbox(&invalid).is_err(), "offset {offset}");
        assert_eq!(invalid.raw_registers, preserved);
    }
}

#[tokio::test]
#[ignore = "Chỉ chạy với AS200_TEST_DATABASE là database development được chỉ định; DML test rollback toàn bộ"]
async fn postgres_inbox_tree_idempotency_constraints_and_rollback() {
    use sqlx::Acquire;
    let pool = crate::database::test_support::pool(2).await;
    let (_, history) = crate::database::migrations::audit(&pool).await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].version,
        crate::database::migrations::INITIAL_VERSION
    );
    // Kiểm tra các truy vấn thực trên schema mới, không thay đổi dữ liệu.
    let request = MeasurementListRequest::default();
    MeasurementRepository::list(&pool, &request).await.unwrap();
    MeasurementRepository::statistics(&pool, &request)
        .await
        .unwrap();
    assert!(
        MeasurementRepository::find_by_id(&pool, i64::MAX)
            .await
            .unwrap()
            .is_none()
    );
    let (before,): (i64,) = sqlx::query_as("SELECT count(*) FROM measurements")
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let key = format!("test-only-rollback-{}", Utc::now().timestamp_micros());
    let (source,): (i64,) =
        sqlx::query_as("INSERT INTO plc_sources(source_key) VALUES($1) RETURNING id")
            .bind(&key)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let unknown = InboxRepository::receive(&mut tx, &input(source, 1, None, "unknown-a"))
        .await
        .unwrap();
    let unknown_b = InboxRepository::receive(&mut tx, &input(source, 1, None, "unknown-b"))
        .await
        .unwrap();
    assert_ne!(unknown.id, unknown_b.id);
    assert_eq!(unknown.processing_status, "SESSION_UNCONFIRMED");
    assert!(
        InboxService::process_in_transaction(&mut tx, unknown.id)
            .await
            .is_err()
    );
    let request = input(
        source,
        1,
        Some("externally-confirmed-fixture-session"),
        "confirmed-a",
    );
    let inbox = InboxRepository::receive(&mut tx, &request).await.unwrap();
    assert_eq!(
        InboxRepository::receive(&mut tx, &request)
            .await
            .unwrap()
            .id,
        inbox.id
    );
    let mut another = request.clone();
    another.ingest_key = "same-record-new-observation".into();
    assert_eq!(
        InboxRepository::receive(&mut tx, &another)
            .await
            .unwrap()
            .id,
        inbox.id
    );
    another.raw_registers[2] = 99;
    assert!(InboxRepository::receive(&mut tx, &another).await.is_err());
    let id = InboxService::process_in_transaction(&mut tx, inbox.id)
        .await
        .unwrap();
    assert_eq!(
        InboxService::process_in_transaction(&mut tx, inbox.id)
            .await
            .unwrap(),
        id
    );
    let stations = StationRepository::load(&mut tx, id).await.unwrap();
    let summary: MeasurementSummary = sqlx::query_as("SELECT m.*,t.result AS tip_result,b.result AS butt_result,t.load AS tip_load,b.load AS butt_load FROM measurements m LEFT JOIN measurement_stations t ON t.measurement_id=m.id AND t.station_type='TIP' LEFT JOIN measurement_stations b ON b.measurement_id=m.id AND b.station_type='BUTT' WHERE m.id=$1")
        .bind(id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(summary.overall_result, MeasurementResult::Ng);
    assert_eq!(summary.tip_result.as_deref(), Some("NG"));
    assert_eq!(summary.butt_result.as_deref(), Some("OK"));
    assert_eq!(
        summary.tip_load.as_ref(),
        Some(&real32_numeric(165.25).unwrap())
    );
    assert_eq!(
        summary.butt_load.as_ref(),
        Some(&real32_numeric(125.5).unwrap())
    );
    let json = serde_json::to_value(Measurement {
        summary,
        stations: stations.clone(),
    })
    .unwrap();
    assert_eq!(json["clubNumber"], 42);
    let push: bigdecimal::BigDecimal = json["stations"][0]["points"][0]["attempts"][0]["pushDist"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(push, real32_numeric(1.25).unwrap());
    assert_eq!(stations.len(), 2);
    assert!(
        stations
            .iter()
            .all(|s| s.points.len() == 4 && s.points.iter().all(|p| p.attempts.len() == 4))
    );
    let state: InboxRecord = sqlx::query_as("SELECT * FROM plc_inbox WHERE id=$1")
        .bind(inbox.id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(state.processing_status, "PROCESSED");
    assert_eq!(state.measurement_id, Some(id));
    // Lỗi INSERT attempt giữa cây dữ liệu phải rollback cả measurement/stations/points.
    let next = InboxRepository::receive(
        &mut tx,
        &input(
            source,
            2,
            Some("externally-confirmed-fixture-session"),
            "confirmed-b",
        ),
    )
    .await
    .unwrap();
    sqlx::raw_sql("CREATE FUNCTION pg_temp.as200_test_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF current_setting('as200.test_failure',true)='on' THEN RAISE EXCEPTION 'injected attempt failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER as200_test_failure BEFORE INSERT ON measurement_attempts FOR EACH ROW EXECUTE FUNCTION pg_temp.as200_test_failure();").execute(&mut *tx).await.unwrap();
    sqlx::query("SELECT set_config('as200.test_failure','on',true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    let mut savepoint = tx.begin().await.unwrap();
    assert!(
        InboxService::process_in_transaction(&mut savepoint, next.id)
            .await
            .is_err()
    );
    savepoint.rollback().await.unwrap();
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM measurements WHERE source_id=$1")
        .bind(source)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let (children,):(i64,)=sqlx::query_as("SELECT count(*) FROM measurement_stations WHERE measurement_id IN (SELECT id FROM measurements WHERE source_id=$1)").bind(source).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(children, 2);
    sqlx::query("SELECT set_config('as200.test_failure','off',true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    InboxService::process_in_transaction(&mut tx, next.id)
        .await
        .unwrap();
    // Database CHECK bảo vệ payload, không chỉ dựa vào validation Rust.
    let mut savepoint = tx.begin().await.unwrap();
    let error = sqlx::query("UPDATE plc_inbox SET raw_registers='[1,2]'::jsonb WHERE id=$1")
        .bind(inbox.id)
        .execute(&mut *savepoint)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    savepoint.rollback().await.unwrap();
    let mut savepoint = tx.begin().await.unwrap();
    assert!(
        sqlx::query("INSERT INTO measurement_attempts(point_id,attempt_number) VALUES($1,5)")
            .bind(stations[0].points[0].id)
            .execute(&mut *savepoint)
            .await
            .is_err()
    );
    savepoint.rollback().await.unwrap();
    // Dọn dữ liệu và trigger chỉ bằng rollback transaction kiểm thử, không DELETE/DROP bảng.
    tx.rollback().await.unwrap();
    let (after,): (i64,) = sqlx::query_as("SELECT count(*) FROM measurements")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    let (sources,): (i64,) = sqlx::query_as("SELECT count(*) FROM plc_sources WHERE source_key=$1")
        .bind(key)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(sources, 0);
}

#[test]
fn inbox_metadata_validation_preserves_invalid_raw_and_accepts_zero_or_nonone_baseline() {
    for (seq, total) in [(-1, Some(1)), (999, Some(1)), (1, None), (1, Some(-1))] {
        let mut inbox = row(input(1, seq, Some("confirmed"), "bad-metadata"));
        inbox.total_count = total;
        let raw = inbox.raw_registers.clone();
        assert!(draft_from_inbox(&inbox).is_err());
        assert_eq!(inbox.raw_registers, raw);
    }
    for seq in [0, 42] {
        assert!(draft_from_inbox(&row(input(1, seq, Some("confirmed"), "baseline"))).is_ok());
    }
}

#[tokio::test]
#[ignore = "DML only on verified golf_report_test: committed fixtures, concurrency and own-backend termination"]
async fn postgres_prompt13_invalid_concurrency_and_connection_recovery() {
    let pool = crate::database::test_support::pool(8).await;
    let key = format!(
        "prompt13-service-{}",
        Utc::now().timestamp_nanos_opt().unwrap()
    );
    let source = SourceRepository::upsert(&pool, &key, None)
        .await
        .unwrap()
        .id;
    let session = format!("session-{key}");
    let mut valid = input(source, 1, Some(&session), "valid");
    for points in [32, 128] {
        for point in 0..4 {
            let bits = (17.5_f32 + point as f32).to_bits();
            let offset = points + 22 * point + 4;
            valid.raw_registers[offset] = bits as u16;
            valid.raw_registers[offset + 1] = (bits >> 16) as u16;
        }
    }
    let inbox = InboxService::receive(&pool, &valid).await.unwrap();
    let id = InboxService::process(&pool, inbox.id).await.unwrap();
    let tree = MeasurementRepository::find_by_id(&pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tree.summary.overall_result, MeasurementResult::Ng);
    assert_eq!(tree.stations.len(), 2);
    for station in &tree.stations {
        assert_eq!(station.points.len(), 4);
        for point in &station.points {
            assert_eq!(point.attempt_count, 4);
            assert_eq!(point.decide_attempt, Some(4));
            assert_eq!(
                point.position,
                Some(real32_numeric(17.5 + (point.point_number - 1) as f32).unwrap())
            );
            assert_eq!(point.attempts.len(), 4);
            for attempt in &point.attempts {
                assert_eq!(
                    attempt.push_dist,
                    Some(real32_numeric(1.25 + (attempt.attempt_number - 1) as f32).unwrap())
                );
                assert_eq!(
                    attempt.bend,
                    Some(real32_numeric(-0.5 - (attempt.attempt_number - 1) as f32).unwrap())
                );
            }
        }
    }
    assert_eq!(
        InboxRepository::find(&pool, inbox.id)
            .await
            .unwrap()
            .processing_status,
        "PROCESSED"
    );
    let mut jobs = tokio::task::JoinSet::new();
    for n in 0..8 {
        let pool = pool.clone();
        let mut request = valid.clone();
        request.ingest_key = format!("concurrent-{n}");
        jobs.spawn(async move {
            let row = InboxService::receive(&pool, &request).await.unwrap();
            (row.id, InboxService::process(&pool, row.id).await.unwrap())
        });
    }
    // Also race creation of a previously unseen identity, not only reads of PROCESSED.
    let mut creation = tokio::task::JoinSet::new();
    for n in 0..8 {
        let pool = pool.clone();
        let mut request = input(source, 2, Some(&session), &format!("create-concurrent-{n}"));
        request.raw_registers[2] = 43;
        creation.spawn(async move {
            let row = InboxService::receive(&pool, &request).await.unwrap();
            (row.id, InboxService::process(&pool, row.id).await.unwrap())
        });
    }
    let mut created_identity = None;
    while let Some(result) =
        tokio::time::timeout(std::time::Duration::from_secs(10), creation.join_next())
            .await
            .unwrap()
    {
        let result = result.unwrap();
        if let Some(expected) = created_identity {
            assert_eq!(result, expected);
        } else {
            created_identity = Some(result);
        }
    }
    while let Some(result) =
        tokio::time::timeout(std::time::Duration::from_secs(10), jobs.join_next())
            .await
            .unwrap()
    {
        assert_eq!(result.unwrap(), (inbox.id, id));
    }
    let counts:(i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM measurements WHERE source_id=$1),(SELECT count(*) FROM measurement_stations WHERE measurement_id=$2),(SELECT count(*) FROM measurement_points p JOIN measurement_stations s ON s.id=p.station_id WHERE s.measurement_id=$2),(SELECT count(*) FROM measurement_attempts a JOIN measurement_points p ON p.id=a.point_id JOIN measurement_stations s ON s.id=p.station_id WHERE s.measurement_id=$2)")
        .bind(source).bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(counts, (2, 2, 8, 32));
    for (n, kind) in [
        "negative-sequence",
        "future-sequence",
        "overall",
        "early-ng",
        "incomplete",
    ]
    .iter()
    .enumerate()
    {
        let seq = 10 + n as i32;
        let mut bad = input(source, seq, Some(&session), kind);
        match n {
            0 => {
                bad.record_seq = -1;
                bad.raw_registers[22] = 65535;
                bad.raw_registers[23] = 65535;
            }
            1 => bad.total_count = Some(1),
            2 => bad.raw_registers[20] = 1,
            3 => {
                bad.raw_registers[195] = 2;
                bad.raw_registers[196] = 2;
            }
            _ => {
                bad.raw_registers[20] = 0;
                bad.raw_registers[120..216].fill(0);
            }
        }
        let held = InboxService::receive(&pool, &bad).await.unwrap();
        let error = InboxService::process(&pool, held.id).await.unwrap_err();
        let saved = InboxRepository::find(&pool, held.id).await.unwrap();
        assert_eq!(
            saved.raw_registers,
            serde_json::to_value(&bad.raw_registers).unwrap()
        );
        assert_eq!(saved.processing_status, "FAILED");
        assert!(saved.measurement_id.is_none());
        println!("INVALID {kind}: inbox={} error={error}", held.id);
    }
    // A staged tree is rolled back by terminating exactly our own test transaction backend.
    let recovery = InboxService::receive(&pool, &input(source, 50, Some(&session), "recovery"))
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let staged = InboxService::process_in_transaction(&mut tx, recovery.id)
        .await
        .unwrap();
    let killed:bool=sqlx::query_scalar("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE pid=$1 AND datname='golf_report_test' AND application_name='golf-report-prompt13-test'")
        .bind(pid).fetch_one(&pool).await.unwrap();
    assert!(killed);
    let error = tx.commit().await.unwrap_err();
    println!("RECOVERY staged={staged} backend={pid} commit_error={error}");
    assert!(
        MeasurementRepository::find_by_id(&pool, staged)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        InboxRepository::find(&pool, recovery.id)
            .await
            .unwrap()
            .processing_status,
        "PENDING"
    );
    assert!(
        MeasurementRepository::find_by_id(&pool, id)
            .await
            .unwrap()
            .is_some()
    );
    let restored = InboxService::process(&pool, recovery.id).await.unwrap();
    assert_eq!(
        InboxService::process(&pool, recovery.id).await.unwrap(),
        restored
    );
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM measurements WHERE source_id=$1")
        .bind(source)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(total, 3);
    let orphan:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM measurement_stations s LEFT JOIN measurements m ON m.id=s.measurement_id WHERE m.id IS NULL)+(SELECT count(*) FROM measurement_points p LEFT JOIN measurement_stations s ON s.id=p.station_id WHERE s.id IS NULL)+(SELECT count(*) FROM measurement_attempts a LEFT JOIN measurement_points p ON p.id=a.point_id WHERE p.id IS NULL)").fetch_one(&pool).await.unwrap();
    assert_eq!(orphan, 0);
    println!(
        "PASS prompt13 source={source} key={key}: valid tree, 8 concurrent duplicates + 8 concurrent new identity, 5 invalid payloads, rollback/reconnect; final measurement=3, orphans=0"
    );
    pool.close().await;
}
