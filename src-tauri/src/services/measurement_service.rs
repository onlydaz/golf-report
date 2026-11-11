use crate::{models::*, repositories::*};
use sqlx::{PgPool, Postgres, Transaction};
pub struct MeasurementService;
impl MeasurementService {
    pub fn validate(input: &MeasurementDraft) -> Result<(), String> {
        if input.source_id <= 0 || input.confirmed_session_id.trim().is_empty() {
            return Err("Phải có nguồn và phiên PLC đã được xác nhận".into());
        }
        if input.product_code.chars().count() > 50 {
            return Err("ProductCode vượt 50 ký tự".into());
        }
        if input.stations.len() != 2
            || !input
                .stations
                .iter()
                .any(|s| s.station_type == StationType::Tip)
            || !input
                .stations
                .iter()
                .any(|s| s.station_type == StationType::Butt)
        {
            return Err("Cần đúng hai station TIP và BUTT".into());
        }
        for station in &input.stations {
            if !(0..=4).contains(&station.point_count)
                || station.ng_point.is_some_and(|p| !(0..=4).contains(&p))
                || station.points.len() != 4
            {
                return Err("Station cần 4 điểm, PointCount/NGPoint từ 0 đến 4".into());
            }
            let mut numbers = std::collections::HashSet::new();
            for point in &station.points {
                if !(1..=4).contains(&point.point_number)
                    || !numbers.insert(point.point_number)
                    || !(0..=4).contains(&point.attempt_count)
                    || point
                        .decide_attempt
                        .is_some_and(|a| a < 1 || a > point.attempt_count)
                {
                    return Err("PointNumber/AttemptCount/DecideAttempt không hợp lệ".into());
                }
                if point.attempts.len() != point.attempt_count as usize {
                    return Err("Số attempt lưu phải bằng AttemptCount; không lưu lượt chưa đo như dữ liệu thật".into());
                }
                let mut attempts = std::collections::HashSet::new();
                for attempt in &point.attempts {
                    if !(1..=point.attempt_count).contains(&attempt.attempt_number)
                        || !attempts.insert(attempt.attempt_number)
                    {
                        return Err("AttemptNumber không phù hợp AttemptCount".into());
                    }
                }
            }
        }
        // Không suy kết quả tổng, NGPoint hay lượt quyết định từ khoảng cách/độ cong.
        Ok(())
    }
    pub async fn insert(
        tx: &mut Transaction<'_, Postgres>,
        input: &MeasurementDraft,
    ) -> Result<i64, String> {
        Self::validate(input)?;
        let id = MeasurementRepository::insert(tx, input).await?;
        for station in &input.stations {
            let station_id = StationRepository::insert(tx, id, station).await?;
            for point in &station.points {
                let point_id = PointRepository::insert(tx, station_id, point).await?;
                for attempt in &point.attempts {
                    AttemptRepository::insert(tx, point_id, attempt).await?;
                }
            }
        }
        Ok(id)
    }
    pub async fn list(
        pool: &PgPool,
        request: MeasurementListRequest,
    ) -> Result<MeasurementListResponse, String> {
        MeasurementRepository::list(pool, &request).await
    }
    pub async fn find_by_id(pool: &PgPool, id: i64) -> Result<Measurement, String> {
        MeasurementRepository::find_by_id(pool, id)
            .await?
            .ok_or_else(|| "Không tìm thấy phép đo".into())
    }
    pub async fn statistics(
        pool: &PgPool,
        request: MeasurementListRequest,
    ) -> Result<MeasurementStatistics, String> {
        MeasurementRepository::statistics(pool, &request).await
    }
}
