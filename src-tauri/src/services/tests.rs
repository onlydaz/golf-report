use super::{
    InboxService, MeasurementService,
    inbox_service::{draft_from_inbox, real32_numeric},
};
use crate::{
    database::test_support::{receipt, registers},
    enums::MeasurementResult,
    models::*,
    repositories::*,
};

#[test]
fn all_zero_publication_creates_unknown_draft_without_inventing_metadata() {
    for total in [0, 1, 2] {
        let mut row = receipt(total);
        row.record_seq = 0;
        row.raw_registers = serde_json::json!(vec![0_u16; 240]);
        let before = row.raw_registers.clone();
        let draft = draft_from_inbox(&row).unwrap();
        assert_eq!(draft.record_seq, 0);
        assert_eq!(draft.publication_count, Some(total));
        assert_eq!(draft.club_number, 0);
        assert_eq!(draft.product_code, "SKU-FROZEN");
        assert_eq!(draft.sku_origin, "QC");
        assert_eq!(draft.overall_result, MeasurementResult::Unmeasured);
        assert!(draft.quality_error.is_some());
        for station in draft.stations {
            assert_eq!(station.load.to_string(), "0");
            assert_eq!(station.result, MeasurementResult::Unmeasured);
            assert!(station.points.iter().all(|p| p.attempts.is_empty()
                && p.position.is_none()
                && p.decide_attempt.is_none()));
        }
        assert_eq!(row.raw_registers, before);
    }
}

#[test]
fn manual_sparse_words_preserve_types_counts_raw_sequence_and_sku_provenance() {
    for offset in [14, 131, 180, 239] {
        let mut row = receipt(1);
        let mut words = vec![0_u16; 240];
        words[offset] = 12;
        row.record_seq = 0;
        row.raw_registers = serde_json::json!(words);
        let draft = draft_from_inbox(&row).unwrap();
        assert_eq!(draft.publication_count, Some(1));
        assert_eq!(draft.record_seq, 0);
        assert_eq!(draft.club_number, 0);
        assert!(draft.stations.iter().all(|s| s.points.is_empty()));
        assert_eq!(row.raw_registers[offset], 12);
        assert!(draft.quality_error.is_some());
    }
    let mut row = receipt(1);
    let mut raw = vec![0_u16; 240];
    raw[22] = 999; // A raw sequence mismatch is not a new publication identity.
    for (offset, number) in [
        (28, 0.0_f32),
        (124, -3.5),
        (132, 10.0),
        (134, 12.0),
        (136, 0.0),
    ] {
        raw[offset] = number.to_bits() as u16;
        raw[offset + 1] = (number.to_bits() >> 16) as u16;
    }
    raw[129] = 1; // Actual one attempt, Result/Decide remain unknown, not invented.
    row.record_seq = 999;
    row.raw_registers = serde_json::json!(raw);
    let draft = draft_from_inbox(&row).unwrap();
    let tip = &draft.stations[0];
    assert_eq!(tip.load.to_string(), "-3.5");
    assert_eq!(tip.points[0].position.as_ref().unwrap().to_string(), "10");
    assert_eq!(
        tip.points[0].attempts[0]
            .push_dist
            .as_ref()
            .unwrap()
            .to_string(),
        "12"
    );
    assert_eq!(
        tip.points[0].attempts[0].bend.as_ref().unwrap().to_string(),
        "0"
    );
    assert_eq!(tip.points[0].decide_attempt, None);
    assert_eq!(tip.points[0].result, MeasurementResult::Unmeasured);
    assert_eq!(draft.record_seq, 999);
    assert_eq!(draft.publication_count, Some(1));

    row.assigned_sku = None;
    let missing = draft_from_inbox(&row).unwrap();
    assert_eq!(missing.sku_origin, "MISSING");
    assert_eq!(missing.product_code, "");
    assert!(missing.quality_error.is_some());

    let mut complete = receipt(1);
    let mut raw = registers(1);
    raw[4..14].fill(0);
    complete.raw_registers = serde_json::json!(raw);
    let draft = draft_from_inbox(&complete).unwrap();
    assert_eq!(draft.product_code, "SKU-FROZEN");
    assert_eq!(draft.sku_origin, "QC");
    assert_eq!(draft.quality_error, None);
    assert_eq!(draft.overall_result, MeasurementResult::Ng);
}
#[test]
fn completed_measurement_preserves_zero_negative_and_positive_reals_in_both_word_orders() {
    for high in [false, true] {
        for value in [0.0_f32, -3.5, 2.25] {
            let mut row = receipt(1);
            let mut raw = registers(1);
            // All *measured* REAL fields use the same value. Padding stays zero.
            // Completion metadata/results remain the PLC fixture's explicit values.
            let mut offsets = vec![];
            for (header, base, count) in [(24, 32, 4), (120, 128, 1)] {
                offsets.push(header + 4);
                for point in 0..count {
                    let p = base + point * 22;
                    offsets.push(p + 4);
                    for attempt in 0..4 {
                        offsets.extend([p + 6 + attempt * 4, p + 8 + attempt * 4]);
                    }
                }
            }
            for offset in &offsets {
                let bits = value.to_bits();
                raw[*offset] = bits as u16;
                raw[*offset + 1] = (bits >> 16) as u16;
            }
            if high {
                for offset in offsets.into_iter().chain([2, 22]) {
                    raw.swap(offset, offset + 1);
                }
                row.word_order = "HIGH".into();
            }
            row.raw_registers = serde_json::to_value(&raw).unwrap();
            let before = row.raw_registers.clone();
            let draft = draft_from_inbox(&row).unwrap();
            MeasurementService::validate(&draft).unwrap();
            assert_eq!(draft.overall_result, MeasurementResult::Ng);
            for station in &draft.stations {
                assert_eq!(station.load, real32_numeric(value).unwrap());
                for point in &station.points {
                    assert_eq!(point.position, Some(real32_numeric(value).unwrap()));
                    assert_eq!(point.attempt_count, 4);
                    assert_eq!(point.decide_attempt, Some(4));
                    assert_eq!(point.attempts.len(), 4);
                    for attempt in &point.attempts {
                        assert_eq!(attempt.push_dist, Some(real32_numeric(value).unwrap()));
                        assert_eq!(attempt.bend, Some(real32_numeric(value).unwrap()));
                    }
                }
            }
            assert_eq!(row.raw_registers, before);
        }
    }
}

#[test]
fn projection_preserves_frozen_sku_results_loads_attempts_and_time() {
    let inbox = receipt(10);
    let draft = draft_from_inbox(&inbox).unwrap();
    assert_eq!(draft.measured_at, inbox.received_at);
    assert_eq!(draft.product_code, "SKU-FROZEN");
    assert_eq!(draft.overall_result, MeasurementResult::Ng);
    assert_eq!(draft.club_number, 42);
    assert_eq!(draft.stations.len(), 2);
    let tip = &draft.stations[0];
    let butt = &draft.stations[1];
    assert_eq!(tip.station_type, StationType::Tip);
    assert_eq!(butt.station_type, StationType::Butt);
    assert_eq!(tip.load.to_string(), "165.25");
    assert_eq!(butt.load.to_string(), "125.5");
    assert_eq!(tip.points.len(), 1);
    assert_eq!(butt.points.len(), 4);
    assert_eq!(tip.points[0].attempts.len(), 4);
    assert_eq!(tip.points[0].decide_attempt, Some(4));
    assert_eq!(
        tip.points[0].attempts[3].bend.as_ref().unwrap().to_string(),
        "-3.5"
    );
    assert_eq!(
        tip.points[0].attempts[0]
            .push_dist
            .as_ref()
            .unwrap()
            .to_string(),
        "1.25"
    );
}
#[test]
fn zero_one_word_and_status_zero_project_as_unknown_without_mutating_raw() {
    for case in 0..3 {
        let mut inbox = receipt(0);
        let mut raw = vec![0_u16; 240];
        if case == 1 {
            raw[180] = 12;
        } // D25200 is low WORD of TIP P3 A1 Bend, not Bend=12.
        if case == 2 {
            raw = registers(0);
            raw[1] = 0;
        }
        inbox.raw_registers = serde_json::to_value(&raw).unwrap();
        let draft = draft_from_inbox(&inbox).unwrap();
        assert_eq!(draft.overall_result, MeasurementResult::Unmeasured);
        assert!(draft.quality_error.is_some());
        assert_eq!(inbox.raw_registers, serde_json::to_value(raw).unwrap());
    }
}
#[test]
fn missing_sku_session_and_publication_identity_have_specific_projection_errors() {
    let mut row = receipt(1);
    row.assigned_sku = None;
    assert_eq!(draft_from_inbox(&row).unwrap().sku_origin, "PLC");
    row = receipt(1);
    row.publication_session = None;
    assert!(
        draft_from_inbox(&row)
            .unwrap_err()
            .contains("SESSION_UNCONFIRMED")
    );
    row = receipt(1);
    row.publication_count = Some(2);
    assert!(draft_from_inbox(&row).unwrap_err().contains("Publication"));
}
#[test]
fn workflow_rejects_extra_tip_points_and_inconsistent_overall_without_rewriting_raw() {
    for offset in [20, 121, 122, 150] {
        let mut row = receipt(1);
        let mut raw = registers(1);
        raw[offset] = if offset == 20 { 1 } else { 2 };
        row.raw_registers = serde_json::to_value(raw).unwrap();
        let before = row.raw_registers.clone();
        let draft = draft_from_inbox(&row).unwrap();
        assert!(draft.quality_error.is_some(), "offset {offset}");
        assert_eq!(draft.overall_result, MeasurementResult::Unmeasured);
        assert_eq!(row.raw_registers, before);
    }
}
#[test]
fn ng_stops_at_first_butt_point_and_keeps_unused_projection_values_null() {
    let mut row = receipt(1);
    let mut raw = registers(1);
    raw[24] = 2;
    raw[25] = 1;
    raw[26] = 1;
    raw[32] = 2;
    raw[54..120].fill(0);
    row.raw_registers = serde_json::to_value(raw).unwrap();
    let draft = draft_from_inbox(&row).unwrap();
    let butt = &draft.stations[1];
    assert_eq!(butt.point_count, 1);
    assert_eq!(butt.points.len(), 4);
    assert_eq!(butt.points[0].position.as_ref().unwrap().to_string(), "0");
    for point in &butt.points[1..] {
        assert_eq!(point.result, MeasurementResult::Unmeasured);
        assert_eq!(point.position, None);
        assert_eq!(point.decide_attempt, None);
        assert!(point.attempts.is_empty());
    }
}
#[test]
fn unmeasured_slots_are_null_and_measured_zero_and_negative_values_remain_real_values() {
    let mut row = receipt(1);
    let mut raw = vec![0_u16; 240];
    raw[0] = 1;
    raw[1] = 1;
    raw[4] = 0x4241;
    raw[22] = 1;
    row.raw_registers = serde_json::to_value(raw).unwrap();
    let draft = draft_from_inbox(&row).unwrap();
    assert_eq!(draft.overall_result, MeasurementResult::Unmeasured);
    for station in draft.stations {
        for p in station.points {
            assert_eq!(p.position, None);
            assert_eq!(p.decide_attempt, None);
            assert!(p.attempts.is_empty());
        }
    }
    assert_eq!(real32_numeric(0.0).unwrap().to_string(), "0");
    assert_eq!(real32_numeric(-3.5).unwrap().to_string(), "-3.5");
    assert_eq!(real32_numeric(0.1).unwrap().to_string(), "0.1");
    assert!(real32_numeric(f32::NAN).is_err());
    assert!(real32_numeric(f32::INFINITY).is_err());
}
#[test]
fn protocol_identity_stays_strict_and_incomplete_metadata_is_unknown() {
    for (seq, total) in [(-1, Some(1)), (999, Some(1)), (1, None), (1, Some(-1))] {
        let mut row = receipt(seq);
        row.total_count = total;
        assert!(draft_from_inbox(&row).is_err());
    }
    for seq in [0, 42] {
        assert!(draft_from_inbox(&receipt(seq)).is_ok());
    }
    for value in [0, 9] {
        let mut row = receipt(1);
        let mut raw = registers(1);
        raw[4] = value;
        row.raw_registers = serde_json::to_value(raw).unwrap();
        assert_eq!(
            draft_from_inbox(&row).unwrap().quality_error.is_some(),
            value != 0
        );
    }
    let mut draft = draft_from_inbox(&receipt(1)).unwrap();
    draft.stations[0].points[0].attempt_count = 3;
    assert!(MeasurementService::validate(&draft).is_err());
    draft.stations[0].points[0].attempt_count = 4;
    draft.stations[0].points[0].decide_attempt = Some(5);
    assert!(MeasurementService::validate(&draft).is_err());
}
#[test]
fn historical_sku_key_remains_readable_without_assigning_current_sku() {
    let mut row = receipt(1);
    row.publication_session = None;
    row.publication_count = None;
    row.session_id = Some("legacy-session".into());
    row.ingest_key = "published-sku-v1:4f4c44:123:1".into();
    assert_eq!(draft_from_inbox(&row).unwrap().product_code, "OLD");
    row.ingest_key = "published-sku-v1:bad:123:1".into();
    assert!(draft_from_inbox(&row).is_err());
}

#[tokio::test]
#[ignore = "Requires explicit verified Neon integration opt-in; never provisions DB"]
async fn postgres_projection_transaction_retry_idempotency_and_history() {
    let pool = crate::database::test_support::pool(3).await;
    let key = crate::database::test_support::source_key("services");
    let source = SourceRepository::upsert(&pool, &key, None).await.unwrap();
    // Fixture setup inserts raw exactly as the schema requires. Worker/store integration separately exercises production receive.
    let row:InboxRecord=sqlx::query_as("INSERT INTO plc_inbox(source_id,ingest_key,record_seq,total_count,buffer_index,raw_registers,word_order,received_at,processing_status,publication_session,publication_count,assigned_sku) VALUES($1,'fixture',1,1,0,$2,'LOW',now(),'PENDING',$3,1,$4) RETURNING *")
        .bind(source.id).bind(serde_json::to_value(registers(1)).unwrap()).bind(crate::database::test_support::session()).bind(crate::database::test_support::sku()).fetch_one(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let rolled_back_id = InboxService::process_in_transaction(&mut tx, row.id)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert!(
        MeasurementRepository::find_by_id(&pool, rolled_back_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        InboxRepository::find(&pool, row.id)
            .await
            .unwrap()
            .measurement_id
            .is_none()
    );
    let a = InboxService::process(&pool, row.id);
    let b = InboxService::process(&pool, row.id);
    let (a, b) = tokio::join!(a, b);
    let id = a.unwrap();
    assert_eq!(b.unwrap(), id);
    // Model a lost successful response: discard the returned id and retry by the same Inbox id.
    assert_eq!(InboxService::process(&pool, row.id).await.unwrap(), id);
    let saved = MeasurementRepository::find_by_id(&pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.stations.len(), 2);
    assert_eq!(
        saved.summary.product_code,
        crate::database::test_support::sku()
    );
    let request = MeasurementListRequest {
        product_code: Some(crate::database::test_support::sku()),
        ..Default::default()
    };
    assert!(
        MeasurementRepository::list(&pool, &request)
            .await
            .unwrap()
            .items
            .iter()
            .any(|m| m.id == id)
    );
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM measurements WHERE source_id=$1")
        .bind(source.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        InboxRepository::find(&pool, row.id)
            .await
            .unwrap()
            .processing_status,
        "PROCESSED"
    );
}
