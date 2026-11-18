use crate::{models::*, repositories::*};
use sqlx::{Postgres, Transaction};
pub struct MeasurementService;
impl MeasurementService {
    pub fn validate(input: &MeasurementDraft) -> Result<(), String> {
        if input.source_id <= 0 || input.confirmed_session_id.trim().is_empty() {
            return Err("Phải có nguồn và phiên PLC đã được xác nhận".into());
        }
        if input.product_code.chars().count() > 50 {
            return Err("ProductCode vượt 50 ký tự".into());
        }
        if let Some(count) = input.publication_count {
            if count < 0 || input.total_count != Some(i64::from(count)) {
                return Err("Measurement publication context differs from receipt".into());
            }
        }
        if input.stations.len() != 2 {
            return Err("Expected TIP and BUTT result registers".into());
        }
        let mut station_types = std::collections::HashSet::new();
        for station in &input.stations {
            if !station_types.insert(station.station_type.as_str())
                || station.point_count.is_some_and(|n| !(0..=4).contains(&n))
                || station.ng_point.is_some_and(|n| !(0..=4).contains(&n))
            {
                return Err("Invalid station identity or scalar bounds".into());
            }
            let limit = if station.station_type == StationType::Tip {
                1
            } else {
                4
            };
            let mut numbers = std::collections::HashSet::new();
            for point in &station.points {
                if !(1..=limit).contains(&point.point_number)
                    || !numbers.insert(point.point_number)
                    || !(0..=4).contains(&point.attempt_count)
                    || point.decide_attempt.is_some_and(|n| !(1..=4).contains(&n))
                {
                    return Err("Invalid point identity or scalar bounds".into());
                }
                let mut attempts = std::collections::HashSet::new();
                for attempt in &point.attempts {
                    if !(1..=4).contains(&attempt.attempt_number)
                        || !attempts.insert(attempt.attempt_number)
                    {
                        return Err("Invalid attempt slot identity".into());
                    }
                }
            }
        }
        // Results and counters are independently supplied by PLC. Never infer
        // Overall, stop-at-NG, retry count, or a decision from these details.
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
}
