use crate::models::*;
use sqlx::{PgConnection, Postgres, Transaction};
pub struct PointRepository;
impl PointRepository {
    pub async fn insert(
        tx: &mut Transaction<'_, Postgres>,
        station_id: i64,
        input: &PointInput,
    ) -> Result<i64, String> {
        let (id,):(i64,) = sqlx::query_as("INSERT INTO measurement_points(station_id,point_number,result,attempt_count,decide_attempt,position) VALUES($1,$2,$3,$4,$5,$6) RETURNING id")
            .bind(station_id).bind(input.point_number).bind(input.result.as_str()).bind(input.attempt_count).bind(input.decide_attempt).bind(&input.position)
            .fetch_one(&mut **tx).await.map_err(|e|format!("Lưu point: {e}"))?;
        Ok(id)
    }
    pub async fn load(
        connection: &mut PgConnection,
        station_id: i64,
    ) -> Result<Vec<MeasurementPoint>, String> {
        let mut points: Vec<MeasurementPoint> = sqlx::query_as(
            "SELECT * FROM measurement_points WHERE station_id=$1 ORDER BY point_number",
        )
        .bind(station_id)
        .fetch_all(&mut *connection)
        .await
        .map_err(|e| e.to_string())?;
        for point in &mut points {
            point.attempts = super::AttemptRepository::load(connection, point.id).await?;
        }
        Ok(points)
    }
}
