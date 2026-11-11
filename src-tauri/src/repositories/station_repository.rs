use crate::models::*;
use sqlx::{PgConnection, Postgres, Transaction};

pub struct StationRepository;
impl StationRepository {
    pub async fn insert(
        tx: &mut Transaction<'_, Postgres>,
        measurement_id: i64,
        input: &StationInput,
    ) -> Result<i64, String> {
        let (id,):(i64,) = sqlx::query_as("INSERT INTO measurement_stations(measurement_id,station_type,result,point_count,ng_point,load) VALUES($1,$2,$3,$4,$5,$6) RETURNING id")
            .bind(measurement_id).bind(input.station_type.as_str()).bind(input.result.as_str()).bind(input.point_count).bind(input.ng_point).bind(&input.load)
            .fetch_one(&mut **tx).await.map_err(|e|format!("Lưu station: {e}"))?;
        Ok(id)
    }
    pub async fn load(
        connection: &mut PgConnection,
        measurement_id: i64,
    ) -> Result<Vec<MeasurementStation>, String> {
        let mut stations: Vec<MeasurementStation> = sqlx::query_as(
            "SELECT * FROM measurement_stations WHERE measurement_id=$1 ORDER BY station_type",
        )
        .bind(measurement_id)
        .fetch_all(&mut *connection)
        .await
        .map_err(|e| e.to_string())?;
        for station in &mut stations {
            station.points = super::PointRepository::load(connection, station.id).await?;
        }
        Ok(stations)
    }
}
