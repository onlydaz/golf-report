use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum MeasurementResult {
    Ok,
    Ng,
    Incomplete,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum TipPositionResult {
    Ok,
    Ng,
    Skipped,
}

impl MeasurementResult {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Ng => "NG",
            Self::Incomplete => "INCOMPLETE",
        }
    }
}

impl TipPositionResult {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Ng => "NG",
            Self::Skipped => "SKIPPED",
        }
    }
}
