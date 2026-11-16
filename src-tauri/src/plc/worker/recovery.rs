//! Restart and source switching use the same v4 receipt/checkpoint proof as ACK.
use super::*;
use std::path::Path;

pub(super) async fn processed_receipt(
    config: &SimulatorConfig,
    delivery: &Delivery,
    pool: &PgPool,
) -> Result<InboxRecord, String> {
    let id = delivery
        .inbox_id
        .ok_or("Bản ghi chưa commit Inbox; không ACK.")?;
    let row = InboxRepository::find(pool, id).await?;
    let key: String = sqlx::query_scalar("SELECT source_key FROM plc_sources WHERE id=$1")
        .bind(row.source_id)
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?;
    if key != config.source_key || !processed_receipt_matches(&row, &delivery.input) {
        return Err("Receipt chưa PROCESSED hoặc source/session/raw không khớp; không ACK.".into());
    }
    let matching: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM measurements WHERE id=$1 AND source_id=$2 AND session_id=$3 AND record_seq=$4)",
    ).bind(row.measurement_id).bind(row.source_id).bind(&row.session_id).bind(row.record_seq)
        .fetch_one(pool).await.map_err(|e| e.to_string())?;
    if !matching {
        return Err("Measurement không khớp receipt; không ACK.".into());
    }
    Ok(row)
}

async fn verify_history(
    config: &SimulatorConfig,
    journal: &Journal,
    pool: &PgPool,
) -> Result<(), String> {
    journal.validate(config)?;
    let progress = journal
        .published_ack
        .as_ref()
        .ok_or("Thiếu ACK progress v4.")?;
    if let Some(receipt) = &progress.confirmed_receipt {
        processed_receipt(config, receipt, pool).await?;
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id JOIN measurements m ON m.id=i.measurement_id AND m.source_id=i.source_id AND m.session_id=i.session_id AND m.record_seq=i.record_seq WHERE s.source_key=$1 AND i.session_id=$2 AND i.record_seq BETWEEN 1 AND $3 AND i.processing_status='PROCESSED' AND i.processed_at IS NOT NULL",
    ).bind(&config.source_key).bind(&journal.session_id).bind(progress.accepted)
        .fetch_one(pool).await.map_err(|e| e.to_string())?;
    if count != i64::from(progress.accepted) {
        return Err(
            "Prefix đã ACK thiếu receipt PROCESSED trong PostgreSQL; giữ checkpoint để đối chiếu."
                .into(),
        );
    }
    Ok(())
}

impl Engine {
    pub(super) async fn verify_published_history(&self, pool: &PgPool) -> Result<(), String> {
        let journal = self.journal.lock().await.clone();
        verify_history(&self.config, &journal, pool).await
    }
}

impl Journal {
    // Only an already closed epoch may isolate its reset diagnostic when switching.
    // The reset stays in this journal and must be confirmed on return to this source.
    fn closed_reset(&self) -> bool {
        self.pause.as_deref() == Some("RESET")
            && self.quarantine.is_empty()
            && self
                .published_ack
                .as_ref()
                .is_some_and(|p| p.accepted > 0 && self.last_total == Some(p.accepted))
            && !self.pending_ack()
    }

    pub(super) fn closed(&self, allow_reset: bool) -> Result<(), String> {
        let progress = self
            .published_ack
            .as_ref()
            .ok_or("Thiếu ACK progress v4.")?;
        if self.pending_ack() {
            return Err(
                "Máy đo còn ACK chưa có bằng chứng hoàn tất. Chưa thể chuyển kết nối.".into(),
            );
        }
        if self.last_total.unwrap_or(0) != progress.accepted
            || !self.deliveries.is_empty()
            || !self.retries.is_empty()
            || self
                .manual_retries
                .iter()
                .any(|r| matches!(r.state.as_str(), "PENDING" | "WAITING"))
        {
            return Err(
                "Máy đo còn dữ liệu chưa lưu/ACK đầy đủ; không tự bỏ qua hoặc tạo phiên mới."
                    .into(),
            );
        }
        if !(allow_reset && self.closed_reset())
            && (self.pause.is_some() || !self.quarantine.is_empty())
        {
            return Err(
                "Máy đo còn cảnh báo hoặc dữ liệu cách ly cần đối chiếu. Chưa thể chuyển kết nối."
                    .into(),
            );
        }
        Ok(())
    }
}

impl WorkerState {
    pub(in crate::plc) async fn verify_source_closed(
        &self,
        file: &Path,
        config: &SimulatorConfig,
        pool: &PgPool,
        for_reset: bool,
        trace: &str,
    ) -> Result<File, String> {
        if self.status.read().await.enabled || !self.tasks.lock().await.is_empty() {
            return Err("Chưa dừng xong thu dữ liệu.".into());
        }
        let path = config.journal_file(file);
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))
            .map_err(|_| "Không thể xác minh khóa Journal của máy đo.".to_string())?;
        lock.try_lock()
            .map_err(|_| "Journal vẫn đang được Worker khác sử dụng.".to_string())?;
        let journal = Journal::load(&path, config)?;
        journal.closed(for_reset)?;
        verify_history(config, &journal, pool).await?;
        let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id WHERE s.source_key=$1 AND (i.processing_status <> 'PROCESSED' OR i.measurement_id IS NULL OR i.processed_at IS NULL)")
            .bind(&config.source_key).fetch_one(pool).await.map_err(|e| e.to_string())?;
        if pending != 0 {
            return Err("Máy đo còn Inbox chưa xử lý; chưa thể chuyển kết nối.".into());
        }
        switch_log(
            trace,
            "JOURNAL_CHECK",
            "PASS",
            "v4=true processed_prefix=true no_pending_ack=true",
        );
        Ok(lock)
    }

    pub(in crate::plc) async fn initialize_separate_journal(
        config: SimulatorConfig,
        path: PathBuf,
    ) -> Result<(), String> {
        if path.exists() || path.with_extension("lock").exists() {
            return Err("Không được ghi đè Journal hoặc khóa cũ.".into());
        }
        // Callers prove a clean baseline and unused identity/session before this call.
        let engine = Engine::open(config, path, WorkerState::default())?;
        let mut journal = engine.journal.lock().await;
        journal.last_total = Some(0);
        engine.persist(&journal)
    }

    pub(in crate::plc) async fn verify_resume(
        &self,
        file: &Path,
        config: &SimulatorConfig,
        historical: bool,
        database: &DatabaseState,
        trace: &str,
    ) -> Result<bool, String> {
        let journal = Journal::load(&config.journal_file(file), config)?;
        let pool = database.pool().await?;
        verify_history(config, &journal, &pool).await?;
        let mut reader = As200Reader::new(config.reader());
        let snapshot = reader
            .poll_published_record()
            .await
            .map_err(|e| e.to_string())?;
        if journal
            .last_total
            .is_some_and(|previous| snapshot.control.total_count < previous)
        {
            // No new epoch while any receipt/intent/checkpoint is unresolved.
            journal.closed(true)?;
            let _lock = if !historical {
                Some(
                    self.verify_source_closed(file, config, &pool, true, trace)
                        .await?,
                )
            } else {
                None
            };
            super::super::connection_settings::read_new_baseline(config).await?;
            return Ok(true); // Eligibility only; caller still asks for explicit reset confirmation.
        }
        if journal.pause.is_some() || !journal.quarantine.is_empty() {
            return Err(
                "Journal còn cảnh báo cần đối chiếu; không tự tiếp tục phiên hoặc bỏ qua dữ liệu."
                    .into(),
            );
        }
        if !snapshot.diagnostics.is_empty() {
            return Err("Dữ liệu máy đo chưa hợp lệ; cần đối chiếu.".into());
        }
        // Pending v4 intents are recovered by observation, never replayed by settings.
        if journal.pending_ack() {
            return Ok(false);
        }
        let accepted = journal
            .published_ack
            .as_ref()
            .ok_or("Thiếu checkpoint v4.")?
            .accepted;
        if snapshot.control.pc_read_count != accepted || snapshot.control.pc_read_done != 0 {
            return Err(
                "Checkpoint/Done trên máy đo không khớp Journal v4; chưa thể tiếp tục.".into(),
            );
        }
        match snapshot.buffer.as_ref().and_then(|b| b.record.as_ref()) {
            None if snapshot.control.data_ready == 0
                && snapshot.control.total_count == accepted => {}
            Some(record)
                if snapshot.control.data_ready == 1
                    && accepted.checked_add(1) == Some(record.record_seq) =>
            {
                if journal.deliveries.iter().any(|d| {
                    d.input.record_seq == record.record_seq
                        && d.input.raw_registers != record.raw_registers
                }) {
                    return Err(
                        "Raw của cùng RecordSeq không khớp Journal; chưa thể tiếp tục.".into(),
                    );
                }
            }
            _ => return Err(
                "Dữ liệu đang công bố có sequence gap hoặc không khớp checkpoint; cần đối chiếu."
                    .into(),
            ),
        }
        Ok(false)
    }

    pub(in crate::plc) async fn ensure_connection_edit_safe(
        &self,
        file: &Path,
        config: &SimulatorConfig,
    ) -> Result<Option<File>, String> {
        if self.status.read().await.enabled
            || self
                .engine
                .lock()
                .await
                .as_ref()
                .and_then(|e| e.upgrade())
                .is_some()
        {
            return Err("Worker chưa dừng an toàn.".into());
        }
        let path = config.journal_file(file);
        if config.mode == "modbus"
            && !config.technical_setup_confirmed
            && !config.enabled
            && !config.ack_enabled
            && !config.pc_heartbeat_enabled
            && config.source_key.is_empty()
            && config.confirmed_session_id.is_none()
            && config.journal_path.is_none()
            && !path.exists()
            && !path.with_extension("lock").exists()
        {
            return Ok(None);
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))
            .map_err(|e| e.to_string())?;
        lock.try_lock().map_err(|e| e.to_string())?;
        let journal = Journal::load(&path, config)?;
        journal.closed(false)?;
        if journal
            .published_ack
            .as_ref()
            .is_some_and(|p| p.accepted != 0)
            || !journal.warnings.is_empty()
            || !journal.previous_session_warnings.is_empty()
        {
            return Err("Journal có lịch sử; dùng luồng chuyển nguồn để đối chiếu, không sửa endpoint trực tiếp.".into());
        }
        Ok(Some(lock))
    }
}
