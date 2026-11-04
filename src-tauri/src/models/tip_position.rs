use crate::enums::TipPositionResult;
use bigdecimal::BigDecimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TipPosition {
    pub position_number: i16,
    pub final_distance: Option<BigDecimal>,
    pub retry_count: Option<i16>,
    pub result: TipPositionResult,
}
