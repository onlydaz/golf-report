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
    pub async fn processed(
        tx: &mut Transaction<'_, Postgres>,
        id: i64,
        measurement_id: i64,
    ) -> Result<(), String> {
        sqlx::query("UPDATE plc_inbox SET processing_status='PROCESSED',measurement_id=$2,error_message=NULL,decode_error=NULL,processed_at=now() WHERE id=$1")
            .bind(id).bind(measurement_id).execute(&mut **tx).await.map_err(|e|e.to_string())?;
        Ok(())
    }
    pub async fn failed(pool: &PgPool, id: i64, error: &str) -> Result<(), String> {
        sqlx::query("UPDATE plc_inbox SET processing_status='FAILED',error_message=$2 WHERE id=$1 AND processing_status IN ('PENDING','FAILED') AND measurement_id IS NULL AND (session_id IS NOT NULL OR publication_session IS NOT NULL)")
            .bind(id).bind(error).execute(pool).await.map_err(|e|e.to_string())?;
        Ok(())
    }
}
