use crate::{enums::MeasurementResult, models::*, repositories::*};
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
            let point_limit = match &station.station_type {
                StationType::Tip => 1,
                StationType::Butt => 4,
            };
            if !(0..=point_limit).contains(&station.point_count)
                || station.ng_point.is_some_and(|p| !(0..=point_limit).contains(&p))
                || station.points.len() != point_limit as usize
            {
                return Err("TIP cần P1; BUTT cần P1–P4. PointCount/NGPoint phải khớp số điểm của trạm.".into());
            }
            let ng_point = station.ng_point.unwrap_or(0);
            let measured = match station.result {
                MeasurementResult::Ok if station.point_count == point_limit && ng_point == 0 => {
                    point_limit
                }
                MeasurementResult::Ng
                    if (1..=point_limit).contains(&ng_point) && station.point_count == ng_point =>
                {
                    ng_point
                }
                MeasurementResult::Unmeasured if station.point_count == 0 && ng_point == 0 => 0,
                _ => return Err("Kết quả trạm không khớp PointCount/NGPoint.".into()),
            };
            let mut numbers = std::collections::HashSet::new();
            for point in &station.points {
                if !(1..=point_limit).contains(&point.point_number)
                    || !numbers.insert(point.point_number)
                    || !(0..=4).contains(&point.attempt_count)
                    || point
                        .decide_attempt
                        .is_some_and(|a| a < 1 || a > point.attempt_count)
                {
                    return Err("PointNumber/AttemptCount/DecideAttempt không hợp lệ".into());
                }
                let expected = if point.point_number > measured {
                    MeasurementResult::Unmeasured
                } else if station.result == MeasurementResult::Ng && point.point_number == measured {
                    MeasurementResult::Ng
                } else {
                    MeasurementResult::Ok
                };
                if point.result != expected || match expected {
                    MeasurementResult::Unmeasured => point.attempt_count != 0 || point.decide_attempt.is_some() || point.position.is_some(),
                    MeasurementResult::Ng => point.attempt_count != 4 || point.decide_attempt != Some(4),
                    MeasurementResult::Ok => !(1..=4).contains(&point.attempt_count) || point.decide_attempt != Some(point.attempt_count),
                } {
                    return Err("Kết quả điểm/lượt quyết định không khớp quy trình đo; điểm bỏ qua không được có dữ liệu đo.".into());
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
        let expected_overall = if input.stations.iter().any(|s| s.result == MeasurementResult::Ng) {
            MeasurementResult::Ng
        } else if input.stations.iter().all(|s| s.result == MeasurementResult::Ok) {
            MeasurementResult::Ok
        } else {
            MeasurementResult::Unmeasured
        };
        if input.overall_result != expected_overall {
            return Err("Overall không khớp kết quả TIP/BUTT PLC công bố.".into());
        }
        // Chỉ đối chiếu kết quả PLC, không tính lại chất lượng từ độ cong/ngưỡng.
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
