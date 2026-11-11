use crate::models::*;
use sqlx::{PgPool, Postgres, Transaction};
pub struct SourceRepository;
impl SourceRepository {
    pub async fn upsert(
        pool: &PgPool,
        key: &str,
        description: Option<&str>,
    ) -> Result<PlcSource, String> {
        if key.trim().is_empty() || key.chars().count() > 100 {
            return Err("SourceKey cần 1–100 ký tự ổn định, không tự suy từ IP".into());
        }
        sqlx::query_as("INSERT INTO plc_sources(source_key,description) VALUES($1,$2) ON CONFLICT(source_key) DO UPDATE SET description=COALESCE(EXCLUDED.description,plc_sources.description) RETURNING *")
            .bind(key).bind(description).fetch_one(pool).await.map_err(|e|e.to_string())
    }
    pub async fn lock(tx: &mut Transaction<'_, Postgres>, source_id: i64) -> Result<(), String> {
        let _: (i64,) = sqlx::query_as("SELECT id FROM plc_sources WHERE id=$1 FOR NO KEY UPDATE")
            .bind(source_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
pub struct InboxRepository;
impl InboxRepository {
    pub async fn find_optional(pool: &PgPool, id: i64) -> Result<Option<InboxRecord>, String> {
        sqlx::query_as("SELECT * FROM plc_inbox WHERE id=$1")
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())
    }
    pub async fn find(pool: &PgPool, id: i64) -> Result<InboxRecord, String> {
        sqlx::query_as("SELECT * FROM plc_inbox WHERE id=$1")
            .bind(id)
            .fetch_one(pool)
            .await
            .map_err(|e| e.to_string())
    }
    pub async fn lock(tx: &mut Transaction<'_, Postgres>, id: i64) -> Result<InboxRecord, String> {
        sqlx::query_as("SELECT * FROM plc_inbox WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut **tx)
            .await
            .map_err(|e| e.to_string())
    }
    pub async fn receive(
        tx: &mut Transaction<'_, Postgres>,
        input: &InboxInput,
    ) -> Result<InboxRecord, String> {
        SourceRepository::lock(tx, input.source_id).await?;
        let existing:Vec<InboxRecord>=sqlx::query_as("SELECT * FROM plc_inbox WHERE source_id=$1 AND (ingest_key=$2 OR ($3::text IS NOT NULL AND session_id=$3 AND record_seq=$4)) ORDER BY id FOR UPDATE")
            .bind(input.source_id).bind(&input.ingest_key).bind(&input.confirmed_session_id).bind(input.record_seq).fetch_all(&mut **tx).await.map_err(|e|e.to_string())?;
        if existing.len() > 1 {
            return Err(
                "IngestKey và định danh phiên trỏ tới hai inbox khác nhau; không tự gộp".into(),
            );
        }
        let raw = serde_json::to_value(&input.raw_registers).map_err(|e| e.to_string())?;
        if let Some(existing) = existing.into_iter().next() {
            if existing.session_id != input.confirmed_session_id
                || existing.record_seq != input.record_seq
                || existing.raw_registers != raw
                || existing.word_order != input.word_order.as_str()
                || (existing.ingest_key == input.ingest_key
                    && (existing.total_count != input.total_count
                        || existing.buffer_index != input.buffer_index
                        || existing.received_at.timestamp_micros()
                            != input.received_at.timestamp_micros()))
            {
                return Err("Xung đột idempotency: cùng delivery/session key nhưng payload khác; không ghi đè dữ liệu gốc".into());
            }
            return Ok(existing);
        }
        sqlx::query_as("INSERT INTO plc_inbox(source_id,session_id,ingest_key,record_seq,total_count,buffer_index,raw_registers,word_order,received_at,processing_status) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING *")
            .bind(input.source_id).bind(&input.confirmed_session_id).bind(&input.ingest_key).bind(input.record_seq).bind(input.total_count).bind(input.buffer_index).bind(raw).bind(input.word_order.as_str()).bind(input.received_at)
            .bind(if input.confirmed_session_id.is_some(){"PENDING"}else{"SESSION_UNCONFIRMED"})
            .fetch_one(&mut **tx).await.map_err(|e|e.to_string())
    }
    pub async fn pending(pool: &PgPool, limit: i64) -> Result<Vec<InboxRecord>, String> {
        sqlx::query_as("SELECT * FROM plc_inbox WHERE processing_status IN ('PENDING','FAILED') ORDER BY received_at,id LIMIT $1")
            .bind(limit.clamp(1,100)).fetch_all(pool).await.map_err(|e|e.to_string())
    }
    pub async fn pending_for_source(
        pool: &PgPool,
        source_id: i64,
        limit: i64,
    ) -> Result<Vec<InboxRecord>, String> {
        sqlx::query_as("SELECT * FROM plc_inbox WHERE source_id=$1 AND processing_status IN ('PENDING','FAILED') AND session_id IS NOT NULL ORDER BY received_at,id LIMIT $2")
            .bind(source_id).bind(limit.clamp(1,100)).fetch_all(pool).await.map_err(|e|e.to_string())
    }
    pub async fn scheduled_for_source(
        pool: &PgPool,
        source_id: i64,
        excluded: &[i64],
    ) -> Result<Vec<InboxRecord>, String> {
        // Mỗi nhóm có suất riêng: FAILED không chiếm hết PENDING và ngược lại.
        sqlx::query_as("SELECT * FROM ((SELECT * FROM plc_inbox WHERE source_id=$1 AND session_id IS NOT NULL AND processing_status='PENDING' AND NOT(id=ANY($2)) ORDER BY received_at,id LIMIT 25) UNION ALL (SELECT * FROM plc_inbox WHERE source_id=$1 AND session_id IS NOT NULL AND processing_status='FAILED' AND NOT(id=ANY($2)) ORDER BY received_at,id LIMIT 25)) scheduled ORDER BY CASE WHEN processing_status='PENDING' THEN 0 ELSE 1 END,received_at,id")
            .bind(source_id).bind(excluded).fetch_all(pool).await.map_err(|e| e.to_string())
    }
    pub async fn processed_for_ids(
        pool: &PgPool,
        source_id: i64,
        ids: &[i64],
    ) -> Result<Vec<InboxRecord>, String> {
        sqlx::query_as("SELECT * FROM plc_inbox WHERE source_id=$1 AND id=ANY($2) AND processing_status='PROCESSED'")
            .bind(source_id).bind(ids).fetch_all(pool).await.map_err(|e| e.to_string())
    }
    pub async fn existing_delivery(
        pool: &PgPool,
        source_id: i64,
        input: &InboxInput,
    ) -> Result<Option<InboxRecord>, String> {
        let rows:Vec<InboxRecord>=sqlx::query_as("SELECT * FROM plc_inbox WHERE source_id=$1 AND (ingest_key=$2 OR ($3::text IS NOT NULL AND session_id=$3 AND record_seq=$4)) ORDER BY id")
            .bind(source_id).bind(&input.ingest_key).bind(&input.confirmed_session_id).bind(input.record_seq).fetch_all(pool).await.map_err(|e|e.to_string())?;
        if rows.len() > 1 {
            return Err("Delivery và session/sequence trỏ tới hai inbox khác nhau".into());
        }
        Ok(rows.into_iter().next())
    }
    pub async fn session_total(
        pool: &PgPool,
        source_id: i64,
        session: &str,
    ) -> Result<Option<i64>, String> {
        let (value,): (Option<i64>,) = sqlx::query_as(
            "SELECT max(total_count) FROM plc_inbox WHERE source_id=$1 AND session_id=$2",
        )
        .bind(source_id)
        .bind(session)
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(value)
    }
    pub async fn processed(
        tx: &mut Transaction<'_, Postgres>,
        id: i64,
        measurement_id: i64,
    ) -> Result<(), String> {
        sqlx::query("UPDATE plc_inbox SET processing_status='PROCESSED',measurement_id=$2,error_message=NULL,processed_at=now() WHERE id=$1")
            .bind(id).bind(measurement_id).execute(&mut **tx).await.map_err(|e|e.to_string())?;
        Ok(())
    }
    pub async fn failed(pool: &PgPool, id: i64, error: &str) -> Result<(), String> {
        sqlx::query("UPDATE plc_inbox SET processing_status='FAILED',error_message=$2 WHERE id=$1 AND processing_status IN ('PENDING','FAILED') AND measurement_id IS NULL AND session_id IS NOT NULL")
            .bind(id).bind(error).execute(pool).await.map_err(|e|e.to_string())?;
        Ok(())
    }
}
