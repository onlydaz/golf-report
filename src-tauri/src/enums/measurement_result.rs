use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum MeasurementResult {
    Unmeasured,
    Ok,
    Ng,
}
impl MeasurementResult {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unmeasured => "UNMEASURED",
            Self::Ok => "OK",
            Self::Ng => "NG",
        }
    }
}
impl TryFrom<String> for MeasurementResult {
    type Error = std::io::Error;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "UNMEASURED" => Ok(Self::Unmeasured),
            "OK" => Ok(Self::Ok),
            "NG" => Ok(Self::Ng),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid AS200 result",
            )),
        }
    }
}
