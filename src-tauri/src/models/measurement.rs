use crate::enums::{MeasurementResult, TipPositionResult};
use crate::models::TipPosition;
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateMeasurementRequest {
    pub product_code: String,
    pub club_number: String,
    pub measured_at: DateTime<Utc>,
    pub weight: BigDecimal,
    pub tip_positions: Vec<TipPosition>,
    pub butt_distance: BigDecimal,
    pub butt_result: MeasurementResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Measurement {
    pub id: i64,
    pub product_code: String,
    pub club_number: String,
    pub measured_at: DateTime<Utc>,
    pub weight: BigDecimal,
    pub tip_positions: Vec<TipPosition>,
    pub tip_result: MeasurementResult,
    pub butt_distance: BigDecimal,
    pub butt_result: MeasurementResult,
    pub overall_result: MeasurementResult,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementListRequest {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub search: Option<String>,
    pub product_code: Option<String>,
    pub club_number: Option<String>,
    pub measured_from: Option<DateTime<Utc>>,
    pub measured_to: Option<DateTime<Utc>>,
    pub tip_result: Option<MeasurementResult>,
    pub butt_result: Option<MeasurementResult>,
    pub result: Option<MeasurementResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementListResponse {
    pub items: Vec<Measurement>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementStatistics {
    pub total_measurements: i64,
    pub total_ok: i64,
    pub total_ng: i64,
    pub pass_rate: f64,
    pub tip_ng: i64,
    pub butt_ng: i64,
    pub daily: Vec<DailyMeasurementStatistics>,
    pub station_results: Vec<StationMeasurementStatistics>,
    pub overall_distribution: Vec<ResultDistribution>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyMeasurementStatistics {
    pub date: String,
    pub total: i64,
    pub ok: i64,
    pub ng: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StationMeasurementStatistics {
    pub station: String,
    pub ok: i64,
    pub ng: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultDistribution {
    pub result: String,
    pub count: i64,
}

pub fn calculate_tip_result(tips: &[TipPosition]) -> MeasurementResult {
    if tips.len() != 4 {
        return MeasurementResult::Incomplete;
    }
    if tips.iter().any(|tip| tip.result == TipPositionResult::Ng) {
        return MeasurementResult::Ng;
    }
    if tips.iter().all(|tip| tip.result == TipPositionResult::Ok) {
        return MeasurementResult::Ok;
    }
    MeasurementResult::Incomplete
}

pub fn calculate_overall_result(tip_result: MeasurementResult, butt_result: MeasurementResult) -> MeasurementResult {
    if tip_result == MeasurementResult::Ng || butt_result == MeasurementResult::Ng {
        MeasurementResult::Ng
    } else if tip_result == MeasurementResult::Ok && butt_result == MeasurementResult::Ok {
        MeasurementResult::Ok
    } else {
        MeasurementResult::Incomplete
    }
}
