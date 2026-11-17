//! Read-only cutover gate. Never repairs, compacts or sends an old ACK intent.
use super::*;
use crate::models::StoredWordOrder;

// Historical delivery shape, read only. No legacy ingestion or ACK implementation.
#[derive(Deserialize)]
struct LegacyInboxInput {
    source_id: i64,
    confirmed_session_id: Option<String>,
    ingest_key: String,
    record_seq: i32,
    total_count: Option<i64>,
    buffer_index: i32,
    raw_registers: Vec<u16>,
    word_order: StoredWordOrder,
    received_at: DateTime<Utc>,
}

pub(super) fn binding(config: &SimulatorConfig) -> String {
    let mut value = format!(
        "published-ack-v1|{}|{}|{}|{}|{:?}|{}|{:?}|as200",
        config.mode,
        config.address,
        config.unit_id,
        config.source_key,
        config.confirmed_session_id,
        config.modbus_address_offset,
        config.word_order
    );
    if config.control_start != as200::map::CONTROL_BASE {
        value.push_str(&format!(
            "|{}|{}|{}|{}",
            config.layout_profile, config.control_start, config.buffer_count, config.record_size
        ));
    }
    value
}

pub(super) async fn baseline(
    config: &SimulatorConfig,
    path: &std::path::Path,
    pool: &PgPool,
    source: i64,
) -> Result<(i32, Option<i64>, i32), String> {
    let fail = "Journal cũ còn dữ liệu/ACK chưa rõ hoặc không tương thích. Giữ nguyên file và PostgreSQL; cần đối chiếu trước khi chuyển đổi.";
    let directory = path
        .ancestors()
        .find(|p| p.join("as200_config.toml").is_file())
        .ok_or(fail)?;
    if path.with_extension("tmp").exists() {
        return Err(fail.into());
    }
    if [
        "as200_capture_journal.json",
        "as200_capture_journal.lock",
        "as200_config.connection-pending",
        "as200_config.connection-draft",
    ]
    .iter()
    .any(|name| directory.join(name).exists())
    {
        return Err(fail.into());
    }
    // Unknown legacy files/directories may contain an outstanding ACK. Only the
    // separate simulator directory is unrelated to the production PLC namespace.
    if let Some(parent) = path.parent().filter(|p| p.exists()) {
        for entry in std::fs::read_dir(parent).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let separate_simulator = parent == directory.join("local-runtime")
                && entry.file_name() == "simulator"
                && entry.path().is_dir();
            if entry.path() != path
                && entry.path() != path.with_extension("lock")
                && !separate_simulator
            {
                return Err(fail.into());
            }
        }
    }
    if !path.exists() {
        let history: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM plc_inbox WHERE source_id=$1) OR EXISTS(SELECT 1 FROM measurements WHERE source_id=$1)")
            .bind(source).fetch_one(pool).await.map_err(|e| e.to_string())?;
        if history {
            return Err(
                "Thiếu trạng thái ACK nhưng còn lịch sử DB; không tự nhận checkpoint từ PLC."
                    .into(),
            );
        }
        return Ok((0, None, 0));
    }
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let session = config.confirmed_session_id.as_deref().ok_or(fail)?;
    if value["version"] != 4
        || value["source_key"].as_str() != Some(config.source_key.as_str())
        || value["session_id"].as_str() != Some(session)
        || !value["pause"].is_null()
    {
        return Err(fail.into());
    }
    let allowed = [
        "version",
        "source_key",
        "session_id",
        "last_total",
        "pause",
        "warnings",
        "deliveries",
        "quarantine",
        "retries",
        "manual_retries",
        "previous_session_warnings",
        "published_ack",
        "ack",
        "session_request",
        "session_archives",
    ];
    if value
        .as_object()
        .is_none_or(|obj| obj.keys().any(|key| !allowed.contains(&key.as_str())))
    {
        return Err(fail.into());
    }
    // The old v4 reader accepted these markers only when empty. Do not write
    // the normalized representation back to the original Journal.
    if value
        .get("ack")
        .is_some_and(|v| v != &serde_json::json!({"binding":"", "accepted":0, "intent":null}))
        || value.get("session_request").is_some_and(|v| !v.is_null())
        || value
            .get("session_archives")
            .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
    {
        return Err(fail.into());
    }
    for field in [
        "deliveries",
        "quarantine",
        "retries",
        "previous_session_warnings",
    ] {
        if value
            .get(field)
            .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
            || (field == "deliveries" && value.get(field).is_none())
        {
            return Err(fail.into());
        }
    }
    if value.get("manual_retries").is_some_and(|items| {
        items.as_array().is_none_or(|items| {
            items
                .iter()
                .any(|item| !matches!(item["state"].as_str(), Some("APPLIED" | "REJECTED")))
        })
    }) {
        return Err(fail.into());
    }
    let p = &value["published_ack"];
    let accepted = p["accepted"]
        .as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .filter(|n| *n >= 0)
        .ok_or(fail)?;
    let last_total = if accepted == 0 && value["last_total"].is_null() {
        0
    } else {
        value["last_total"]
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or(fail)?
    };
    let published_next = accepted > 0
        && accepted.checked_add(1) == Some(last_total)
        && p["intent"]["phase"] == "ACK_CONFIRMED";
    if p["binding"].as_str() != Some(binding(config).as_str())
        || !(last_total == accepted || published_next)
    {
        return Err(fail.into());
    }
    let receipt = &p["confirmed_receipt"];
    if accepted == 0 && !receipt.is_null() {
        return Err(fail.into());
    }
    let anchor = if accepted == 0 && receipt.is_null() {
        None
    } else {
        let input: LegacyInboxInput =
            serde_json::from_value(receipt["input"].clone()).map_err(|_| fail)?;
        let id = receipt["inbox_id"].as_i64().ok_or(fail)?;
        let row = InboxRepository::find(pool, id).await?;
        if row.source_id != source
            || (input.source_id != 0 && input.source_id != source)
            || row.session_id != input.confirmed_session_id
            || row.ingest_key != input.ingest_key
            || row.record_seq != input.record_seq
            || row.total_count != Some(i64::from(accepted))
            || row.total_count != input.total_count
            || row.buffer_index != 0
            || input.buffer_index != 0
            || row.word_order != config.word_order.as_str()
            || input.word_order != config.word_order
            || row.raw_registers
                != serde_json::to_value(&input.raw_registers).map_err(|e| e.to_string())?
            || input.raw_registers.len() != 240
            || row.received_at.timestamp_micros() != input.received_at.timestamp_micros()
        {
            return Err(fail.into());
        }
        let raw_key = format!(
            "raw-published-v1:ack:{}:{session}:{accepted}",
            session.len()
        );
        if row.session_id.as_deref() == Some(session) {
            if row.record_seq != accepted
                || !matches!(
                    row.processing_status.as_str(),
                    "PENDING" | "FAILED" | "PROCESSED"
                )
            {
                return Err(fail.into());
            }
            if row.processing_status == "PROCESSED" {
                let linked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM measurements WHERE id=$1 AND source_id=$2 AND session_id=$3 AND record_seq=$4)")
                    .bind(row.measurement_id).bind(source).bind(session).bind(accepted).fetch_one(pool).await.map_err(|e| e.to_string())?;
                if !linked || row.processed_at.is_none() {
                    return Err(fail.into());
                }
            }
        } else if row.session_id.is_some()
            || row.ingest_key != raw_key
            || row.processing_status != "SESSION_UNCONFIRMED"
            || row.measurement_id.is_some()
            || row.processed_at.is_some()
        {
            return Err(fail.into());
        }
        if !p["intent"].is_null()
            && (p["intent"]["phase"] != "ACK_CONFIRMED"
                || p["intent"]["target"].as_i64() != Some(i64::from(accepted))
                || p["intent"]["inbox_id"].as_i64() != Some(id)
                || p["intent"]["observed_done_high"] != true
                || p["intent"]["raw_registers"] != row.raw_registers)
        {
            return Err(fail.into());
        }
        Some(id)
    };
    if accepted == 0 && !p["intent"].is_null() {
        return Err(fail.into());
    }
    // Full prefix verification happens once, at cutover, never on idle polls.
    let prefix = format!("raw-published-v1:ack:{}:{session}:", session.len());
    let count: i64 = sqlx::query_scalar("SELECT count(DISTINCT target) FROM (SELECT i.record_seq::bigint AS target FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id WHERE s.source_key=$1 AND i.session_id=$2 AND i.record_seq BETWEEN 1 AND $3 AND ((i.processing_status IN ('PENDING','FAILED') AND i.measurement_id IS NULL AND i.processed_at IS NULL) OR (i.processing_status='PROCESSED' AND i.processed_at IS NOT NULL AND EXISTS(SELECT 1 FROM measurements m WHERE m.id=i.measurement_id AND m.source_id=i.source_id AND m.session_id=i.session_id AND m.record_seq=i.record_seq))) UNION ALL SELECT i.total_count AS target FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id WHERE s.source_key=$1 AND i.session_id IS NULL AND i.total_count BETWEEN 1 AND $3 AND i.ingest_key=$4 || i.total_count::text AND i.processing_status='SESSION_UNCONFIRMED' AND i.measurement_id IS NULL AND i.processed_at IS NULL AND i.buffer_index=0) receipts")
        .bind(&config.source_key).bind(session).bind(accepted).bind(prefix).fetch_one(pool).await.map_err(|e| e.to_string())?;
    if count != i64::from(accepted) {
        return Err(fail.into());
    }
    let unaccounted: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM plc_inbox WHERE source_id=$1 AND (publication_session IS NOT NULL OR (session_id=$2 AND (record_seq>$3 OR total_count>$3)) OR (session_id IS NULL AND ingest_key=$4 || total_count::text AND total_count>$3))) OR EXISTS(SELECT 1 FROM measurements WHERE source_id=$1 AND session_id=$2 AND (record_seq>$3 OR total_count>$3))")
        .bind(source).bind(session).bind(accepted)
        .bind(format!("raw-published-v1:ack:{}:{session}:", session.len()))
        .fetch_one(pool).await.map_err(|e| e.to_string())?;
    if unaccounted {
        return Err("DB còn receipt ngoài checkpoint Journal hoặc receipt của backend mới nhưng thiếu trạng thái ACK; giữ nguyên để đối chiếu.".into());
    }
    Ok((accepted, anchor, last_total))
}
