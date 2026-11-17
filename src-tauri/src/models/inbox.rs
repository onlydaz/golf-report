use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum StoredWordOrder {
    Low,
    High,
}
impl StoredWordOrder {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "LOW",
            Self::High => "HIGH",
        }
    }
}
#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct InboxRecord {
    pub publication_session: Option<String>,
    pub publication_count: Option<i32>,
    pub assigned_sku: Option<String>,
    pub decode_error: Option<String>,
    pub id: i64,
    pub source_id: i64,
    pub session_id: Option<String>,
    pub ingest_key: String,
    pub record_seq: i32,
    pub total_count: Option<i64>,
    pub buffer_index: i32,
    pub raw_registers: serde_json::Value,
    pub word_order: String,
    pub received_at: DateTime<Utc>,
    pub processing_status: String,
    pub measurement_id: Option<i64>,
    pub error_message: Option<String>,
    pub processed_at: Option<DateTime<Utc>>,
}
#[derive(Debug, Clone, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PlcSource {
    pub id: i64,
    pub source_key: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
}
