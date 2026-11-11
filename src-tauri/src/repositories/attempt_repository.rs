use crate::models::*;
use sqlx::{PgConnection, Postgres, Transaction};
pub struct AttemptRepository;
impl AttemptRepository {
    pub async fn insert(
        tx: &mut Transaction<'_, Postgres>,
        point_id: i64,
        input: &AttemptInput,
    ) -> Result<(), String> {
        sqlx::query("INSERT INTO measurement_attempts(point_id,attempt_number,push_dist,bend) VALUES($1,$2,$3,$4)")
            .bind(point_id).bind(input.attempt_number).bind(&input.push_dist).bind(&input.bend)
            .execute(&mut **tx).await.map_err(|e|format!("Lưu attempt: {e}"))?;
        Ok(())
    }
    pub async fn load(
        connection: &mut PgConnection,
        point_id: i64,
    ) -> Result<Vec<MeasurementAttempt>, String> {
        sqlx::query_as(
            "SELECT * FROM measurement_attempts WHERE point_id=$1 ORDER BY attempt_number",
        )
        .bind(point_id)
        .fetch_all(connection)
        .await
        .map_err(|e| e.to_string())
    }
}
