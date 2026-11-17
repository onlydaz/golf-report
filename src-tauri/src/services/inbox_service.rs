use super::MeasurementService;
use crate::{
    enums::MeasurementResult,
    models::*,
    plc::as200::{
        WordOrder, decode,
        model::{PlcResult, PlcStation},
    },
    repositories::*,
};
use bigdecimal::BigDecimal;
use sqlx::PgPool;

pub(crate) const RAW_UNDECODED_KEY: &str = "raw-published-v1:";
pub(crate) const LEGACY_RAW_ZERO_KEY: &str = "raw-zero-test-v1:";
pub(crate) const LEGACY_RAW_INVALID_KEY: &str = "raw-invalid-v1:";
pub(crate) const RAW_UNINITIALIZED_KEY: &str = "raw-uninitialized-v1:";

// TotalCount identifies the publication; RecordSeq in the raw words is never rewritten.
pub(crate) fn raw_ack_prefix(session: &str) -> String {
    format!("{RAW_UNDECODED_KEY}ack:{}:{session}:", session.len())
}

pub(crate) fn validate_operator_sku(sku: &str) -> Result<(), String> {
    if sku.is_empty() || sku.len() > 50 || sku != sku.trim()
        || !sku.bytes().all(|b| (32..=126).contains(&b))
    {
        return Err("SKU cần 1–50 ký tự ASCII in được, không có khoảng trắng ở đầu/cuối.".into());
    }
    Ok(())
}

// The immutable delivery key is already persisted in both Journal and Inbox.
// Carry the assigned SKU there without altering raw PLC words or the DB schema.
pub(crate) fn sku_delivery_key(sku: &str, nonce: i64, sequence: i32) -> Result<String, String> {
    validate_operator_sku(sku)?;
    let encoded: String = sku.bytes().map(|b| format!("{b:02x}")).collect();
    Ok(format!("published-sku-v1:{encoded}:{nonce}:{sequence}"))
}

pub(crate) fn assigned_sku(key: &str, sequence: i32) -> Result<Option<String>, String> {
    let Some(rest) = key.strip_prefix("published-sku-v1:") else { return Ok(None); };
    let parts: Vec<_> = rest.split(':').collect();
    if parts.len() != 3 || parts[0].len() % 2 != 0 || !parts[0].is_ascii()
        || parts[1].parse::<i64>().is_err() || parts[2].parse::<i32>().ok() != Some(sequence)
    {
        return Err("SKU delivery không hợp lệ; giữ nguyên Inbox/Journal để đối chiếu.".into());
    }
    let bytes = (0..parts[0].len()).step_by(2)
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
fn station(value: &PlcStation, station_type: StationType) -> Result<StationInput, String> {
    let mut points = Vec::new();
    let point_limit = match &station_type {
        StationType::Tip => 1,
        StationType::Butt => 4,
    };
    for (index, point) in value.points.iter().take(point_limit).enumerate() {
        let mut attempts = Vec::new();
        for attempt in point.attempts.iter().take(point.attempt_count as usize) {
            attempts.push(AttemptInput {
                attempt_number: i32::from(attempt.number),
                push_dist: Some(real32_numeric(attempt.push_dist)?),
                bend: Some(real32_numeric(attempt.bend)?),
            });
        }
        points.push(PointInput {
            point_number: (index + 1) as i32,
            result: result(point.result),
            attempt_count: i32::from(point.attempt_count),
            decide_attempt: if point.decide_attempt == 0 {
                None
            } else {
                Some(i32::from(point.decide_attempt))
            },
            // Workflow validation already proved unused slots contain no data.
            // Keep only business point identities (TIP 1, BUTT 4); padding is not a measurement.
            position: if index < value.point_count as usize && point.attempt_count > 0 {
                Some(real32_numeric(point.position)?)
            } else {
                None
            },
            attempts,
        });
    }
    Ok(StationInput {
        station_type,
        result: result(value.result),
        point_count: i32::from(value.point_count),
        ng_point: Some(i32::from(value.ng_point)),
        load: real32_numeric(value.load)?,
        points,
    })
}
pub fn draft_from_inbox(inbox: &InboxRecord) -> Result<MeasurementDraft, String> {
    if inbox.ingest_key.starts_with(RAW_UNDECODED_KEY)
        || inbox.ingest_key.starts_with(LEGACY_RAW_ZERO_KEY)
        || inbox.ingest_key.starts_with(LEGACY_RAW_INVALID_KEY)
        || inbox.ingest_key.starts_with(RAW_UNINITIALIZED_KEY)
    {
        return Err("Raw đã nhận nhưng chưa giải mã đủ; giữ nguyên bản ghi, không tạo measurement hoặc receipt ACK.".into());
    }
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
    record.validate_workflow()?;
    if !record.valid_unacknowledged || record.record_seq != inbox.record_seq {
        return Err("Status bit 0 hoặc RecordSeq không khớp inbox".into());
    }
    let input = MeasurementDraft {
        source_id: inbox.source_id,
        confirmed_session_id: session.clone(),
        record_seq: record.record_seq,
        total_count: inbox.total_count,
        product_code: assigned_sku(&inbox.ingest_key, inbox.record_seq)?.unwrap_or(record.product_code),
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
pub struct InboxService;
impl InboxService {
    pub async fn receive(pool: &PgPool, input: &InboxInput) -> Result<InboxRecord, String> {
        if input.raw_registers.len() != 240
            || !(0..=1).contains(&input.buffer_index)
            || input.ingest_key.trim().is_empty()
            || input.ingest_key.chars().count() > 200
            || input
                .confirmed_session_id
                .as_ref()
                .is_some_and(|s| s.trim().is_empty())
        {
            return Err(
                "Inbox cần 240 WORD, buffer 0/1, delivery key hợp lệ và không dùng session rỗng"
                    .into(),
            );
        }
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        let inbox = InboxRepository::receive(&mut tx, input).await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(inbox)
    }
    // Chỉ caller đã xác nhận session thực tế mới gọi; không tự suy phiên từ counter reset.
    pub async fn confirm_session(pool: &PgPool, id: i64, session: &str) -> Result<(), String> {
        if session.trim().is_empty() {
            return Err("Phiên đã xác nhận không được rỗng".into());
        }
        let source = InboxRepository::find(pool, id).await?.source_id;
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        SourceRepository::lock(&mut tx, source).await?;
        let inbox = InboxRepository::lock(&mut tx, id).await?;
        if inbox.ingest_key.starts_with(RAW_UNDECODED_KEY)
            || inbox.ingest_key.starts_with(LEGACY_RAW_ZERO_KEY)
            || inbox.ingest_key.starts_with(LEGACY_RAW_INVALID_KEY)
            || inbox.ingest_key.starts_with(RAW_UNINITIALIZED_KEY)
        {
            return Err("Không gán phiên đo cho raw chưa giải mã; chỉ delivery hợp lệ mới được xử lý thành measurement.".into());
        }
        if let Some(existing) = inbox.session_id {
            if existing == session {
                tx.commit().await.map_err(|e| e.to_string())?;
                return Ok(());
            }
            return Err("Không thay đổi session đã gán. Cần đối chiếu thủ công".into());
        }
        sqlx::query("UPDATE plc_inbox SET session_id=$2,processing_status='PENDING' WHERE id=$1")
            .bind(id)
            .bind(session)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("Xác nhận session bị xung đột, không tự gộp inbox: {e}"))?;
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(())
    }
    pub async fn process_in_transaction(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        id: i64,
    ) -> Result<i64, String> {
        let inbox = InboxRepository::lock(tx, id).await?;
        if let Some(measurement_id) = inbox.measurement_id {
            return Ok(measurement_id);
        }
        let draft = draft_from_inbox(&inbox)?;
        if draft.overall_result == MeasurementResult::Unmeasured
            || draft
                .stations
                .iter()
                .any(|s| s.result == MeasurementResult::Unmeasured)
        {
            return Err(
                "Bản ghi chưa hoàn tất; giữ inbox để đối chiếu, không tạo measurement".into(),
            );
        }
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
