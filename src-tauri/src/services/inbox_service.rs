use super::MeasurementService;
use crate::{
    enums::MeasurementResult,
    models::*,
    plc::as200::{
        WordOrder, decode, map,
        map::record as r,
        model::{PlcMeasurementPoint, PlcResult, PlcStation},
    },
    repositories::*,
};
use bigdecimal::BigDecimal;
use sqlx::PgPool;

pub(crate) fn validate_operator_sku(sku: &str) -> Result<(), String> {
    if sku.is_empty()
        || sku.len() > 50
        || sku != sku.trim()
        || !sku.bytes().all(|b| (32..=126).contains(&b))
    {
        return Err("SKU cần 1–50 ký tự ASCII in được, không có khoảng trắng ở đầu/cuối.".into());
    }
    Ok(())
}

// Read the assigned SKU in historical Inbox keys; new receipts use assigned_sku.
pub(crate) fn assigned_sku(key: &str, sequence: i32) -> Result<Option<String>, String> {
    let Some(rest) = key.strip_prefix("published-sku-v1:") else {
        return Ok(None);
    };
    let parts: Vec<_> = rest.split(':').collect();
    if parts.len() != 3
        || parts[0].len() % 2 != 0
        || !parts[0].is_ascii()
        || parts[1].parse::<i64>().is_err()
        || parts[2].parse::<i32>().ok() != Some(sequence)
    {
        return Err("SKU delivery không hợp lệ; giữ nguyên Inbox/Journal để đối chiếu.".into());
    }
    let bytes = (0..parts[0].len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&parts[0][i..i + 2], 16))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "SKU delivery lỗi mã hóa; không tự thay bằng SKU hiện hành.".to_string())?;
    let sku = String::from_utf8(bytes).map_err(|_| "SKU delivery lỗi ký tự.".to_string())?;
    validate_operator_sku(&sku)?;
    Ok(Some(sku))
}

pub fn real32_numeric(value: f32) -> Result<BigDecimal, String> {
    if !value.is_finite() {
        return Err("REAL32 NaN/Infinity không thể lưu như phép đo".into());
    }
    // Decimal ngắn nhất round-trip về cùng REAL32; không chia 100, không áp hệ số.
    value
        .to_string()
        .parse()
        .map_err(|_| "Không chuyển được REAL32 sang NUMERIC".into())
}
fn result(value: PlcResult) -> MeasurementResult {
    match value {
        PlcResult::Unmeasured => MeasurementResult::Unmeasured,
        PlcResult::Ok => MeasurementResult::Ok,
        PlcResult::Ng => MeasurementResult::Ng,
    }
}
fn point(value: &PlcMeasurementPoint, number: usize) -> Result<PointInput, String> {
    let attempts = value
        .attempts
        .iter()
        .filter(|a| {
            i16::from(a.number) <= value.attempt_count
                || i16::from(a.number) == value.decide_attempt
                || a.push_dist != 0.0
                || a.bend != 0.0
        })
        .map(|a| {
            Ok(AttemptInput {
                attempt_number: i32::from(a.number),
                push_dist: Some(real32_numeric(a.push_dist)?),
                bend: Some(real32_numeric(a.bend)?),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let has_detail = value.attempt_count > 0
        || value.decide_attempt > 0
        || value.result != PlcResult::Unmeasured
        || value.position != 0.0
        || !attempts.is_empty();
    Ok(PointInput {
        point_number: number as i32,
        result: result(value.result),
        attempt_count: i32::from(value.attempt_count),
        decide_attempt: (value.decide_attempt != 0).then_some(i32::from(value.decide_attempt)),
        position: if has_detail {
            Some(real32_numeric(value.position)?)
        } else {
            None
        },
        attempts,
    })
}
fn station(value: &PlcStation, station_type: StationType) -> Result<StationInput, String> {
    let limit = if station_type == StationType::Tip {
        1
    } else {
        4
    };
    Ok(StationInput {
        station_type,
        result: result(value.result),
        point_count: Some(i32::from(value.point_count)),
        ng_point: Some(i32::from(value.ng_point)),
        load: Some(real32_numeric(value.load)?),
        points: value
            .points
            .iter()
            .take(limit)
            .enumerate()
            .map(|(i, p)| point(p, i + 1))
            .collect::<Result<_, _>>()?,
    })
}
// A malformed detail must not erase a valid result register. Missing scalar
// projections stay NULL/absent with diagnostics; the original WORDs remain in Inbox.
fn partial_station(raw: &[u16], order: WordOrder, station_type: StationType) -> StationInput {
    let (base, points_base, limit) = if station_type == StationType::Tip {
        (r::TIP, r::TIP_POINTS, 1)
    } else {
        (r::BUTT, r::BUTT_POINTS, 4)
    };
    let bounded = |offset| {
        decode::int16(raw, offset)
            .ok()
            .filter(|n| (0..=4).contains(n))
            .map(i32::from)
    };
    StationInput {
        station_type,
        result: decoded_result(raw, base),
        point_count: bounded(base + 1),
        ng_point: bounded(base + 2),
        load: decode::real32(raw, base + 4, order)
            .ok()
            .and_then(|v| real32_numeric(v).ok()),
        points: (0..limit)
            .filter_map(|i| {
                decode::point(raw, points_base + i * r::POINT_SIZE, order)
                    .ok()
                    .and_then(|p| point(&p, i + 1).ok())
                    .or_else(|| partial_point(raw, points_base + i * r::POINT_SIZE, order, i + 1))
            })
            .collect(),
    }
}
fn partial_point(raw: &[u16], base: usize, order: WordOrder, number: usize) -> Option<PointInput> {
    let point_result = decode::int16(raw, base).and_then(decode::result).ok()?;
    let count = decode::int16(raw, base + 1)
        .ok()
        .filter(|n| (0..=4).contains(n))?;
    let decide = decode::int16(raw, base + 2)
        .ok()
        .filter(|n| (1..=4).contains(n));
    let numeric = |offset| {
        decode::real32(raw, offset, order)
            .ok()
            .and_then(|v| real32_numeric(v).ok())
    };
    let mut attempts = Vec::new();
    for index in 0..4 {
        let offset = base + 6 + index * 4;
        let number = (index + 1) as i32;
        if number <= i32::from(count)
            || decide.map(i32::from) == Some(number)
            || raw[offset..offset + 4].iter().any(|word| *word != 0)
        {
            attempts.push(AttemptInput {
                attempt_number: number,
                push_dist: numeric(offset),
                bend: numeric(offset + 2),
            });
        }
    }
    let has_detail = count > 0
        || decide.is_some()
        || point_result != PlcResult::Unmeasured
        || !attempts.is_empty()
        || raw[base + 4..base + 6].iter().any(|word| *word != 0);
    Some(PointInput {
        point_number: number as i32,
        result: result(point_result),
        attempt_count: i32::from(count),
        decide_attempt: decide.map(i32::from),
        position: if has_detail { numeric(base + 4) } else { None },
        attempts,
    })
}
fn decoded_result(raw: &[u16], offset: usize) -> MeasurementResult {
    decode::int16(raw, offset)
        .and_then(decode::result)
        .map(result)
        .unwrap_or(MeasurementResult::Unmeasured)
}
fn legacy_draft_from_inbox(inbox: &InboxRecord) -> Result<MeasurementDraft, String> {
    let session = inbox
        .session_id
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .ok_or("SESSION_UNCONFIRMED: cần xác nhận phiên ngoài hệ thống; không tự tạo session")?;
    let raw: Vec<u16> =
        serde_json::from_value(inbox.raw_registers.clone()).map_err(|e| e.to_string())?;
    let order = match inbox.word_order.as_str() {
        "LOW" => WordOrder::LowWordFirst,
        "HIGH" => WordOrder::HighWordFirst,
        _ => return Err("WordOrder không hợp lệ".into()),
    };
    let record = decode::record(
        &raw,
        usize::try_from(inbox.buffer_index).map_err(|_| "Buffer index không hợp lệ")?,
        order,
    )
    .map_err(|e| e.to_string())?;
    record.validate_metadata(inbox.total_count)?;
    record.validate_product_code()?;
    if !record.valid_unacknowledged || record.record_seq != inbox.record_seq {
        return Err("Status bit 0 hoặc RecordSeq không khớp inbox".into());
    }
    if !record.complete() {
        return Err("Legacy receipt is not a completed measurement".into());
    }
    let input = MeasurementDraft {
        publication_count: None,
        quality_error: None,
        sku_origin: "LEGACY".into(),
        source_id: inbox.source_id,
        confirmed_session_id: session.clone(),
        record_seq: record.record_seq,
        total_count: inbox.total_count,
        product_code: assigned_sku(&inbox.ingest_key, inbox.record_seq)?
            .unwrap_or(record.product_code),
        club_number: record.stick_no,
        measured_at: inbox.received_at,
        overall_result: result(record.overall_result),
        stations: vec![
            station(&record.tip, StationType::Tip)?,
            station(&record.butt, StationType::Butt)?,
        ],
    };
    MeasurementService::validate(&input)?;
    Ok(input)
}
/// Native receipts use publication identity, independently of raw RecordSeq.
pub fn draft_from_inbox(inbox: &InboxRecord) -> Result<MeasurementDraft, String> {
    if inbox.publication_session.is_none() {
        return legacy_draft_from_inbox(inbox);
    }
    let session = inbox
        .publication_session
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .ok_or("SESSION_UNCONFIRMED")?;
    let count = inbox
        .publication_count
        .filter(|c| *c >= 0)
        .ok_or("Invalid publication counter")?;
    if inbox.total_count != Some(i64::from(count)) || inbox.source_id <= 0 {
        return Err("Publication identity does not match receipt".into());
    }
    let raw: Vec<u16> =
        serde_json::from_value(inbox.raw_registers.clone()).map_err(|e| e.to_string())?;
    if raw.len() != map::RECORD_SIZE || inbox.buffer_index != 0 {
        return Err("Receipt requires 240 WORD".into());
    }
    let order = match inbox.word_order.as_str() {
        "LOW" => WordOrder::LowWordFirst,
        "HIGH" => WordOrder::HighWordFirst,
        _ => return Err("Invalid word order".into()),
    };
    let seq = decode::dint32(&raw, r::SEQUENCE, order).map_err(|e| e.to_string())?;
    if seq != inbox.record_seq {
        return Err("Raw RecordSeq differs from receipt".into());
    }
    let decoded = decode::publication_record(&raw, 0, order);
    let product_code = inbox.assigned_sku.clone().unwrap_or_else(|| {
        decode::product_code(&raw[r::PRODUCT_CODE..r::PRODUCT_CODE + r::PRODUCT_WORDS])
            .unwrap_or_default()
    });
    let sku_origin = if inbox.assigned_sku.is_some() {
        "QC"
    } else if !product_code.is_empty() {
        "PLC"
    } else {
        "MISSING"
    };
    let mut quality_error = decode::record(&raw, 0, order)
        .map_err(|e| e.to_string())
        .and_then(|mut r| {
            r.validate_metadata(inbox.total_count)?;
            if !r.product_code.is_empty()
                && !r.product_code.bytes().all(|b| (32..=126).contains(&b))
            {
                return Err("PLC ProductCode contains invalid characters".into());
            }
            // SKU is receipt metadata, never rewritten into the raw WORD array.
            r.product_code = product_code.clone();
            r.validate_product_code()?;
            if !r.complete() || r.record_seq != count {
                return Err("Incomplete results or RecordSeq/publication mismatch".into());
            }
            Ok(())
        })
        .err();
    if product_code.is_empty() {
        quality_error = Some("SKU not assigned at receipt; PLC ProductCode empty".into());
    }
    let mut stations = if let Ok(record) = &decoded {
        vec![
            station(&record.tip, StationType::Tip)?,
            station(&record.butt, StationType::Butt)?,
        ]
    } else {
        vec![
            partial_station(&raw, order, StationType::Tip),
            partial_station(&raw, order, StationType::Butt),
        ]
    };
    if quality_error.is_some() {
        for station in &mut stations {
            station.points.retain(|p| {
                p.result != MeasurementResult::Unmeasured
                    || p.attempt_count > 0
                    || p.decide_attempt.is_some()
                    || p.position.is_some()
                    || !p.attempts.is_empty()
            });
        }
    }
    // D25040 is authoritative even if completion/metadata/detail diagnostics exist.
    let overall_result = decoded_result(&raw, r::OVERALL_RESULT);
    let draft = MeasurementDraft {
        source_id: inbox.source_id,
        confirmed_session_id: session.clone(),
        record_seq: seq,
        publication_count: Some(count),
        quality_error,
        sku_origin: sku_origin.into(),
        total_count: inbox.total_count,
        product_code,
        club_number: decode::dint32(&raw, r::STICK_NO, order).map_err(|e| e.to_string())?,
        measured_at: inbox.received_at,
        overall_result,
        stations,
    };
    MeasurementService::validate(&draft)?;
    Ok(draft)
}

pub struct InboxService;
impl InboxService {
    pub async fn process_in_transaction(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        id: i64,
    ) -> Result<i64, String> {
        let inbox = InboxRepository::lock(tx, id).await?;
        if let Some(measurement_id) = inbox.measurement_id {
            return Ok(measurement_id);
        }
        let draft = draft_from_inbox(&inbox)?;
        let measurement_id = MeasurementService::insert(tx, &draft).await?;
        InboxRepository::processed(tx, id, measurement_id).await?;
        Ok(measurement_id)
    }
    async fn process_transaction(pool: &PgPool, id: i64) -> Result<i64, String> {
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        let measurement_id = Self::process_in_transaction(&mut tx, id).await?;
        tx.commit()
            .await
            .map_err(|e| format!("Commit chưa được xác nhận, retry cùng inbox id: {e}"))?;
        Ok(measurement_id)
    }
    pub async fn process(pool: &PgPool, id: i64) -> Result<i64, String> {
        match Self::process_transaction(pool, id).await {
            Ok(id) => Ok(id),
            Err(error) => {
                // Transaction đã rollback; không ghi FAILED đè lên một lần retry đã commit.
                if let Err(mark) = InboxRepository::failed(pool, id, &error).await {
                    return Err(format!(
                        "{error}; không cập nhật được trạng thái retry: {mark}"
                    ));
                }
                Err(error)
            }
        }
    }
}
