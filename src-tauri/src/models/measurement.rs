use crate::enums::MeasurementResult;
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum StationType {
    Tip,
    Butt,
}
impl StationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tip => "TIP",
            Self::Butt => "BUTT",
        }
    }
}
impl TryFrom<String> for StationType {
    type Error = std::io::Error;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "TIP" => Ok(Self::Tip),
            "BUTT" => Ok(Self::Butt),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid station",
            )),
        }
    }
}
#[derive(Debug, Clone)]
pub struct AttemptInput {
    pub attempt_number: i32,
    pub push_dist: Option<BigDecimal>,
    pub bend: Option<BigDecimal>,
}
#[derive(Debug, Clone)]
pub struct PointInput {
    pub point_number: i32,
    pub result: MeasurementResult,
    pub attempt_count: i32,
    pub decide_attempt: Option<i32>,
    pub position: Option<BigDecimal>,
    pub attempts: Vec<AttemptInput>,
}
#[derive(Debug, Clone)]
pub struct StationInput {
    pub station_type: StationType,
    pub result: MeasurementResult,
    pub point_count: i32,
    pub ng_point: Option<i32>,
    pub load: BigDecimal,
    pub points: Vec<PointInput>,
}
#[derive(Debug, Clone)]
pub struct MeasurementDraft {
    pub source_id: i64,
    pub confirmed_session_id: String,
    pub record_seq: i32,
    pub total_count: Option<i64>,
    pub product_code: String,
    pub club_number: i32,
    pub measured_at: DateTime<Utc>,
    pub overall_result: MeasurementResult,
    pub stations: Vec<StationInput>,
}
#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementAttempt {
    pub id: i64,
    pub point_id: i64,
    pub attempt_number: i32,
    pub push_dist: Option<BigDecimal>,
    pub bend: Option<BigDecimal>,
}
#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementPoint {
    pub id: i64,
    pub station_id: i64,
    pub point_number: i32,
    #[sqlx(try_from = "String")]
    pub result: MeasurementResult,
    pub attempt_count: i32,
    pub decide_attempt: Option<i32>,
    pub position: Option<BigDecimal>,
    #[sqlx(skip)]
    pub attempts: Vec<MeasurementAttempt>,
}
#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementStation {
    pub id: i64,
    pub measurement_id: i64,
    #[sqlx(try_from = "String")]
    pub station_type: StationType,
    #[sqlx(try_from = "String")]
    pub result: MeasurementResult,
    pub point_count: i32,
    pub ng_point: Option<i32>,
    pub load: BigDecimal,
    pub created_at: DateTime<Utc>,
    #[sqlx(skip)]
    pub points: Vec<MeasurementPoint>,
}
#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementSummary {
    pub id: i64,
    pub source_id: i64,
    pub session_id: Option<String>,
    pub record_seq: i32,
    pub total_count: Option<i64>,
    pub product_code: String,
    pub club_number: i32,
    pub measured_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    #[sqlx(try_from = "String")]
    pub overall_result: MeasurementResult,
    pub tip_result: Option<String>,
    pub tip_load: Option<BigDecimal>,
    pub butt_result: Option<String>,
    pub butt_load: Option<BigDecimal>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Measurement {
    #[serde(flatten)]
    pub summary: MeasurementSummary,
    pub stations: Vec<MeasurementStation>,
}
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementListRequest {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub search: Option<String>,
    pub product_code: Option<String>,
    pub club_number: Option<i32>,
    pub measured_from: Option<DateTime<Utc>>,
    /// Exclusive upper timestamp bound; selected calendar end becomes next-day midnight.
    pub measured_to: Option<DateTime<Utc>>,
    pub tip_result: Option<MeasurementResult>,
    pub butt_result: Option<MeasurementResult>,
    pub result: Option<MeasurementResult>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementListResponse {
    pub items: Vec<MeasurementSummary>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}
#[derive(Debug, Clone, Serialize)]
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
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct DailyMeasurementStatistics {
    pub date: String,
    pub total: i64,
    pub ok: i64,
    pub ng: i64,
}
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct StationMeasurementStatistics {
    pub station: String,
    pub ok: i64,
    pub ng: i64,
}
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ResultDistribution {
    pub result: String,
    pub count: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementQualityStatistics {
    pub retested_clubs: i64,
    pub butt_histogram: Vec<BendHistogramBin>,
    pub points: Vec<PointQualityStatistics>,
    pub point_summary: Vec<PointSummaryStatistics>,
    #[serde(default)]
    pub half_hour: Vec<HalfHourStatistics>,
    #[serde(default)]
    pub demo_limits: Option<ReportDemoLimits>,
    #[serde(default)]
    pub demo_histogram: Vec<ClassifiedBendBin>,
    #[serde(default)]
    pub butt_summary: Option<BendDistributionSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HalfHourStatistics {
    pub at: DateTime<Utc>,
    pub ok: i64,
    pub ng: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReportDemoLimits {
    pub lower: String,
    pub upper: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifiedBendBin {
    pub lower: String,
    pub upper: String,
    pub count: i64,
    pub zone: String,
    pub lower_inclusive: bool,
    pub upper_inclusive: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BendDistributionSummary {
    pub count: i64,
    pub mean: Option<String>,
    pub stddev: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointSummaryStatistics {
    pub station: String,
    pub point_number: i32,
    pub position: Option<String>,
    pub load_avg: Option<String>,
    pub push_dist_avg: Option<String>,
    pub bend_min: Option<String>,
    pub bend_avg: Option<String>,
    pub bend_max: Option<String>,
    pub retested: i64,
    pub ng: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointQualityStatistics {
    pub product_code: String,
    pub station: String,
    pub point_number: i32,
    pub position: Option<String>,
    pub measured: i64,
    pub ng: i64,
    pub retested: i64,
    pub load_avg: Option<String>,
    pub push_dist_avg: Option<String>,
    pub bend_count: i64,
    pub bend_min: Option<String>,
    pub bend_avg: Option<String>,
    pub bend_max: Option<String>,
    pub bend_stddev: Option<String>,
    pub histogram: Vec<BendHistogramBin>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BendHistogramBin {
    pub lower: String,
    pub upper: String,
    pub count: i64,
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementSkuSummary {
    pub product_code: String,
    pub first_measured_at: DateTime<Utc>,
    pub last_measured_at: DateTime<Utc>,
    pub total: i64,
    pub ok: i64,
    pub ng: i64,
    pub butt_ng: i64,
    pub tip_ng: i64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementSkuPage {
    pub items: Vec<MeasurementSkuSummary>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementSkuOptions {
    pub items: Vec<String>,
    pub has_more: bool,
}
#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct HistoryDecisionPoint {
    pub measurement_id: i64,
    pub station: String,
    pub point_number: i32,
    pub bend: Option<BigDecimal>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementHistoryItem {
    #[serde(flatten)]
    pub summary: MeasurementSummary,
    pub decision_points: Vec<HistoryDecisionPoint>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementHistoryPage {
    pub items: Vec<MeasurementHistoryItem>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}
