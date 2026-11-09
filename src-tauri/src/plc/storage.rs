use crate::{
    models::CreateMeasurementRequest, repositories::MeasurementRepository,
    services::MeasurementService,
};
use sqlx::PgPool;

// Lưu con trỏ bền vững; khi quan sát sequence giảm thì bắt đầu thế hệ mới.
pub async fn observe(pool: &PgPool, source: &str, sequence: u16) -> Result<i64, String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query(
        "INSERT INTO plc_sources(source, last_sequence) VALUES ($1,$2) ON CONFLICT DO NOTHING",
    )
    .bind(source)
    .bind(i32::from(sequence))
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    let (last, mut generation): (i32, i64) = sqlx::query_as(
        "SELECT last_sequence, generation FROM plc_sources WHERE source=$1 FOR UPDATE",
    )
    .bind(source)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    if i32::from(sequence) < last {
        generation += 1;
    }
    sqlx::query("UPDATE plc_sources SET last_sequence=$2, generation=$3 WHERE source=$1")
        .bind(source)
        .bind(i32::from(sequence))
        .bind(generation)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(generation)
}

pub async fn enqueue(
    pool: &PgPool,
    source: &str,
    generation: i64,
    sequence: u16,
    request: &CreateMeasurementRequest,
) -> Result<(), String> {
    sqlx::query("INSERT INTO plc_inbox(source,generation,sequence_id,payload) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING")
        .bind(source).bind(generation).bind(i32::from(sequence)).bind(serde_json::to_value(request).map_err(|e| e.to_string())?)
        .execute(pool).await.map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn pending(
    pool: &PgPool,
    source: &str,
) -> Result<Option<(i64, i32, serde_json::Value)>, String> {
    sqlx::query_as("SELECT generation,sequence_id,payload FROM plc_inbox WHERE source=$1 AND measurement_id IS NULL ORDER BY received_at LIMIT 1")
        .bind(source).fetch_optional(pool).await.map_err(|e| e.to_string())
}

pub async fn save(
    pool: &PgPool,
    source: &str,
    generation: i64,
    sequence: i32,
) -> Result<(i64, CreateMeasurementRequest), String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let (payload, saved): (serde_json::Value, Option<i64>) = sqlx::query_as("SELECT payload,measurement_id FROM plc_inbox WHERE source=$1 AND generation=$2 AND sequence_id=$3 FOR UPDATE")
        .bind(source).bind(generation).bind(sequence).fetch_one(&mut *tx).await.map_err(|e| e.to_string())?;
    let request: CreateMeasurementRequest =
        serde_json::from_value(payload).map_err(|e| e.to_string())?;
    if let Some(id) = saved {
        tx.commit().await.map_err(|e| e.to_string())?;
        return Ok((id, request));
    }
    let (tip, overall) = MeasurementService::validate(&request)?;
    let id = MeasurementRepository::insert(
        &mut tx,
        &request.product_code,
        &request.club_number,
        request.measured_at,
        &request.weight,
        tip,
        request.butt_result,
        &request.butt_distance,
        overall,
        &request.tip_positions,
    )
    .await?;
    sqlx::query("UPDATE plc_inbox SET measurement_id=$4 WHERE source=$1 AND generation=$2 AND sequence_id=$3")
        .bind(source).bind(generation).bind(sequence).bind(id).execute(&mut *tx).await.map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok((id, request))
}
