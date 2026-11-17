//! PostgreSQL owns the committed raw receipt and the single outstanding ACK attempt.
use super::*;
use sqlx::FromRow;

#[derive(Clone, FromRow)]
pub(super) struct Progress {
    pub source_id: i64,
    pub session_id: String,
    pub binding: String,
    pub accepted: i32,
    pub last_total: i32,
    pub anchor_inbox_id: Option<i64>,
    pub pending_inbox_id: Option<i64>,
    pub ack_attempted: bool,
    pub done_started: Option<bool>,
    pub done_observed: bool,
    pub blocked_reason: Option<String>,
}

pub(super) async fn progress(pool: &PgPool, source: i64) -> Result<Option<Progress>, String> {
    sqlx::query_as("SELECT * FROM plc_acquisition WHERE source_id=$1")
        .bind(source)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())
}

impl Engine {
    pub(super) async fn block(&self, pool: &PgPool, reason: &str) -> Result<(), String> {
        sqlx::query("UPDATE plc_acquisition SET blocked_reason=COALESCE(blocked_reason,$2) WHERE source_id=$1")
            .bind(self.source_id).bind(reason).execute(pool).await.map_err(|e| e.to_string())?;
        let mut status = self.state.status.write().await;
        status.pause_reason = Some(reason.into());
        status.ack_status = "RECONCILIATION_REQUIRED";
        Ok(())
    }
    pub(super) async fn receive(
        &self,
        pool: &PgPool,
        p: &Progress,
        snapshot: &PublishedRecordSnapshot,
        replace_zero: Option<i64>,
    ) -> Result<InboxRecord, String> {
        let buffer = snapshot
            .buffer
            .as_ref()
            .ok_or("Thiếu 240 WORD khi DataReady=1.")?;
        let total = snapshot.control.total_count;
        let raw = serde_json::to_value(&buffer.raw_registers).map_err(|e| e.to_string())?;
        let seq = decode::dint32(
            &buffer.raw_registers,
            as200::map::record::SEQUENCE,
            self.config.reader().word_order,
        )
        .map_err(|e| e.to_string())?;
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        SourceRepository::lock(&mut tx, p.source_id).await?;
        let current: Progress =
            sqlx::query_as("SELECT * FROM plc_acquisition WHERE source_id=$1 FOR UPDATE")
                .bind(p.source_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
        if current.accepted != p.accepted
            || current.session_id != p.session_id
            || current.binding != p.binding
            || current.ack_attempted
            || current.pending_inbox_id != replace_zero
        {
            return Err("Trạng thái nhận đã đổi; giữ receipt hiện hành, chưa thay pending.".into());
        }
        if let Some(zero_id) = replace_zero {
            let c = &snapshot.control;
            let zero = InboxRepository::lock(&mut tx, zero_id).await?;
            if current.accepted != 0
                || current.anchor_inbox_id.is_some()
                || current.last_total > 1
                || current.done_started != Some(false)
                || current.done_observed
                || current.blocked_reason.is_some()
                || zero.source_id != p.source_id
                || zero.publication_session.as_deref() != Some(p.session_id.as_str())
                || zero.publication_count != Some(0)
                || zero.total_count != Some(0)
                || zero.word_order != self.config.word_order.as_str()
                || zero.buffer_index != 0
                || c.total_count != 1
                || c.pc_read_count != 0
                || c.pc_read_done != 0
                || c.data_ready != 1
            {
                return Err("Chưa chứng minh được raw counter 0 chưa ACK và publication 1 kế tiếp; không thay identity/checkpoint.".into());
            }
        }
        let existing: Option<InboxRecord> = sqlx::query_as(
            "SELECT * FROM plc_inbox WHERE source_id=$1 AND publication_session=$2 AND publication_count=$3 FOR UPDATE")
            .bind(p.source_id).bind(&p.session_id).bind(total).fetch_optional(&mut *tx).await.map_err(|e| e.to_string())?;
        let row = if let Some(row) = existing {
            if row.raw_registers != raw
                || row.word_order != self.config.word_order.as_str()
                || row.record_seq != seq
                || row.total_count != Some(i64::from(total))
            {
                sqlx::query("UPDATE plc_acquisition SET blocked_reason='PUBLICATION_CONFLICT' WHERE source_id=$1")
                    .bind(p.source_id).execute(&mut *tx).await.map_err(|e| e.to_string())?;
                tx.commit().await.map_err(|e| e.to_string())?;
                return Err("CONFLICT: cùng định danh công bố nhưng raw khác; giữ bản gốc, không ACK hoặc gộp dữ liệu.".into());
            }
            row
        } else {
            sqlx::query_as("INSERT INTO plc_inbox(source_id,session_id,ingest_key,record_seq,total_count,buffer_index,raw_registers,word_order,received_at,processing_status,publication_session,publication_count,assigned_sku) VALUES($1,NULL,$2,$3,$4,0,$5,$6,now(),'SESSION_UNCONFIRMED',$7,$8,$9) RETURNING *")
                .bind(p.source_id).bind(format!("publication-v1:{}:{}:{total}", p.session_id.len(), p.session_id))
                .bind(seq).bind(i64::from(total)).bind(raw).bind(self.config.word_order.as_str())
                .bind(&p.session_id).bind(total).bind(&self.config.operator_sku)
                .fetch_one(&mut *tx).await.map_err(|e| e.to_string())?
        };
        // Receipt and pending pointer commit atomically. Counter 0 is kept, never renamed to 1.
        let changed = sqlx::query("UPDATE plc_acquisition SET pending_inbox_id=$2,last_total=GREATEST(last_total,$3),done_started=false WHERE source_id=$1 AND pending_inbox_id IS NOT DISTINCT FROM $4 AND NOT ack_attempted")
            .bind(p.source_id).bind(row.id).bind(total).bind(replace_zero).execute(&mut *tx).await.map_err(|e| e.to_string())?;
        if changed.rows_affected() != 1 {
            return Err("Còn receipt khác chưa đối chiếu; chưa ACK.".into());
        }
        tx.commit().await.map_err(|e| {
            format!("Commit raw chưa được xác nhận; đọc lại cùng publication, không ACK: {e}")
        })?;
        Ok(row)
    }

    pub(super) async fn receipt(
        &self,
        pool: &PgPool,
        id: i64,
        p: &Progress,
    ) -> Result<InboxRecord, String> {
        let row = InboxRepository::find(pool, id).await?;
        if row.source_id != p.source_id
            || row.publication_session.as_deref() != Some(p.session_id.as_str())
            || row.total_count != row.publication_count.map(i64::from)
            || row.buffer_index != 0
            || row.word_order != self.config.word_order.as_str()
            || serde_json::from_value::<Vec<u16>>(row.raw_registers.clone())
                .map_err(|e| e.to_string())?
                .len()
                != 240
        {
            return Err("Receipt không khớp nguồn/phiên/map/raw; không ACK.".into());
        }
        Ok(row)
    }
}
