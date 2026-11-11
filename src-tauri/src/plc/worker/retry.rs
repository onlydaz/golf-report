//! Retry bền vững trong journal; không sửa raw, identity hoặc state PROCESSED.
use crate::{models::InboxRecord, repositories::InboxRepository, services::InboxService};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::future::Future;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct RetryEntry {
    pub inbox_id: i64,
    pub source_id: i64,
    pub session_id: Option<String>,
    pub attempts: u32,
    pub next_attempt_at: DateTime<Utc>,
    pub blocked: bool,
    pub error: String,
}
pub(super) fn excluded(
    entries: &[RetryEntry],
    source_id: i64,
    now: DateTime<Utc>,
    max: u32,
) -> Vec<i64> {
    entries
        .iter()
        .filter(|e| {
            e.source_id == source_id && (e.blocked || e.attempts >= max || e.next_attempt_at > now)
        })
        .map(|e| e.inbox_id)
        .collect()
}
pub(super) fn reserve(
    entries: &mut Vec<RetryEntry>,
    row: &InboxRecord,
    now: DateTime<Utc>,
    base_ms: u64,
    cap_ms: u64,
) {
    let index = entries
        .iter()
        .position(|e| e.inbox_id == row.id && e.source_id == row.source_id)
        .unwrap_or_else(|| {
            entries.push(RetryEntry {
                inbox_id: row.id,
                source_id: row.source_id,
                session_id: row.session_id.clone(),
                attempts: 0,
                next_attempt_at: now,
                blocked: false,
                error: String::new(),
            });
            entries.len() - 1
        });
    let entry = &mut entries[index];
    entry.attempts += 1;
    let delay = base_ms
        .saturating_mul(1u64 << entry.attempts.saturating_sub(1).min(20))
        .min(cap_ms);
    entry.next_attempt_at = now + chrono::Duration::milliseconds(delay as i64);
    entry.error =
        "Đang xử lý; nếu ứng dụng đóng, chờ đến lịch retry để đối chiếu lại cùng inbox".into();
}
pub(super) trait InboxBackend: Sync {
    fn completed(
        &self,
        source_id: i64,
        ids: Vec<i64>,
    ) -> impl Future<Output = Result<Vec<InboxRecord>, String>> + Send;
    fn candidates(
        &self,
        source_id: i64,
        excluded: Vec<i64>,
    ) -> impl Future<Output = Result<Vec<InboxRecord>, String>> + Send;
    fn fail(&self, id: i64, error: &str) -> impl Future<Output = Result<(), String>> + Send;
    fn process(&self, id: i64) -> impl Future<Output = Result<InboxRecord, String>> + Send;
}
pub(super) struct PostgresBackend<'a>(pub &'a sqlx::PgPool);
impl InboxBackend for PostgresBackend<'_> {
    async fn completed(&self, source_id: i64, ids: Vec<i64>) -> Result<Vec<InboxRecord>, String> {
        InboxRepository::processed_for_ids(self.0, source_id, &ids).await
    }
    async fn candidates(
        &self,
        source_id: i64,
        excluded: Vec<i64>,
    ) -> Result<Vec<InboxRecord>, String> {
        InboxRepository::scheduled_for_source(self.0, source_id, &excluded).await
    }
    async fn fail(&self, id: i64, error: &str) -> Result<(), String> {
        InboxRepository::failed(self.0, id, error).await
    }
    async fn process(&self, id: i64) -> Result<InboxRecord, String> {
        InboxService::process(self.0, id).await?;
        InboxRepository::find(self.0, id).await
    }
}

pub(super) trait RetryLookup: Sync {
    fn find(&self, id: i64) -> impl Future<Output = Result<Option<InboxRecord>, String>> + Send;
}
impl RetryLookup for PostgresBackend<'_> {
    async fn find(&self, id: i64) -> Result<Option<InboxRecord>, String> {
        InboxRepository::find_optional(self.0, id).await
    }
}
