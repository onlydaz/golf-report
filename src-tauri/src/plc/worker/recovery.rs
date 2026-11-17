//! Restart recovery verifies committed raw receipts and the v4 ACK checkpoint.
use super::*;

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
    let input = &delivery.input;
    if key != config.source_key || row.session_id != input.confirmed_session_id
        || row.ingest_key != input.ingest_key || row.record_seq != input.record_seq
        || row.total_count != input.total_count || row.buffer_index != input.buffer_index
        || row.word_order != input.word_order.as_str()
        || row.received_at.timestamp_micros() != input.received_at.timestamp_micros()
        || serde_json::to_value(&input.raw_registers).map_err(|e| e.to_string())? != row.raw_registers
    {
        return Err("Receipt PostgreSQL không khớp source/session/counter/raw; không ACK.".into());
    }
    if delivery.raw_publication() {
        let session = config.confirmed_session_id.as_deref().ok_or("Thiếu Session.")?;
        if row.ingest_key != format!("{}{}", crate::services::inbox_service::raw_ack_prefix(session), delivery.ack_target())
            || row.session_id.is_some() || row.measurement_id.is_some() || row.processed_at.is_some()
            || row.processing_status != "SESSION_UNCONFIRMED" || delivery.ack_target() <= 0
        {
            return Err("Receipt raw không khớp định danh công bố; không ACK.".into());
        }
        return Ok(row);
    }
    if !matches!(row.processing_status.as_str(), "PENDING" | "FAILED" | "PROCESSED") {
        return Err("Inbox chưa có receipt thu dữ liệu hợp lệ; không ACK.".into());
    }
    // Decode/measurement failures do not undo the committed 240 WORD receipt.
    if row.processing_status != "PROCESSED" {
        if row.measurement_id.is_some() || row.processed_at.is_some() {
            return Err("Trạng thái measurement/receipt mâu thuẫn; cần đối chiếu.".into());
        }
        return Ok(row);
    }
    if !processed_receipt_matches(&row, input) {
        return Err("Receipt PROCESSED thiếu bằng chứng measurement; cần đối chiếu.".into());
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
        "SELECT count(DISTINCT target) FROM (SELECT i.record_seq::bigint AS target FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id WHERE s.source_key=$1 AND i.session_id=$2 AND i.record_seq BETWEEN 1 AND $3 AND ((i.processing_status IN ('PENDING','FAILED') AND i.measurement_id IS NULL AND i.processed_at IS NULL) OR (i.processing_status='PROCESSED' AND i.processed_at IS NOT NULL AND EXISTS(SELECT 1 FROM measurements m WHERE m.id=i.measurement_id AND m.source_id=i.source_id AND m.session_id=i.session_id AND m.record_seq=i.record_seq))) UNION ALL SELECT i.total_count AS target FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id WHERE s.source_key=$1 AND i.session_id IS NULL AND i.total_count BETWEEN 1 AND $3 AND i.ingest_key=$4 || i.total_count::text AND i.processing_status='SESSION_UNCONFIRMED' AND i.measurement_id IS NULL AND i.processed_at IS NULL AND i.buffer_index=0) receipts",
    ).bind(&config.source_key).bind(&journal.session_id).bind(progress.accepted)
        .bind(crate::services::inbox_service::raw_ack_prefix(journal.session_id.as_deref().ok_or("Thiếu Session.")?))
        .fetch_one(pool).await.map_err(|e| e.to_string())?;
    if count != i64::from(progress.accepted) {
        return Err(
            "Prefix đã ACK thiếu receipt raw đã commit trong PostgreSQL; giữ checkpoint để đối chiếu."
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

    // Only the old "ready + empty RAM" startup diagnostic is recoverable here.
    // An intent (even Prepared), receipt, counter history or unknown diagnostic
    // prevents this path. No reset, replacement session or ACK is performed.
    pub(super) async fn try_resume_initial_raw_pause(
        &self, snapshot: &PublishedRecordSnapshot, pool: &PgPool,
    ) -> Result<(), String> {
        let saved = {
            let journal = self.journal.lock().await;
            if journal.pause.as_deref() != Some("PUBLISHED_ACK_RECONCILIATION") { return Ok(()); }
            journal.clone()
        };
        saved.validate(&self.config)?;
        let progress = saved.published_ack.as_ref().ok_or("Thiếu ACK progress v4.")?;
        if progress.accepted != 0 || progress.intent.is_some() || progress.confirmed_receipt.is_some()
            || saved.last_total.is_some_and(|n| n != 0) || !saved.deliveries.is_empty()
            || !saved.retries.is_empty() || !saved.manual_retries.is_empty() || !saved.previous_session_warnings.is_empty()
            || saved.quarantine.is_empty()
        { return Ok(()); }
        let original = |error: &str| error.starts_with("PUBLISHED_DIAGNOSTIC:") && error.contains("Status bit0");
        if !saved.quarantine.iter().any(|q| original(&q.error))
            || saved.quarantine.iter().any(|q| {
                q.source_key != self.config.source_key || q.session_id != saved.session_id
                    || q.address != self.config.address || q.unit_id != self.config.unit_id
                    || q.modbus_address_offset != self.config.modbus_address_offset || q.word_order != self.config.word_order
                    || q.buffer_index != 0 || q.record_seq != Some(0) || q.total_count != 0
                    || q.raw_registers.len() != as200::map::RECORD_SIZE || q.raw_registers.iter().any(|w| *w != 0)
                    || !(original(&q.error) || q.error.starts_with("RAW_ZERO_TEST_PENDING:") || q.error.starts_with("RAW_ZERO_TEST_SAVED:")
                        || q.error.starts_with("RAW_UNDECODED_PENDING:") || q.error.starts_with("RAW_UNDECODED_SAVED:"))
                    || !q.control.as_ref().is_some_and(|c| c.format_version == 1 && c.record_size == 240 && c.buffer_count == 2
                        && c.total_count == 0 && c.pc_read_count == 0 && c.pc_read_done == 0 && c.data_ready == 1)
            })
            || saved.warnings.iter().any(|w| !(w.starts_with("PUBLISHED_DIAGNOSTIC:")
                || w.starts_with("PUBLISHED_ACK: PUBLISHED_DIAGNOSTIC:")
                || w.starts_with("PUBLISHED_ACK: Journal v4 đang dừng bảo vệ dữ liệu:")
                || w.starts_with("PUBLISHED_ACK: Đã lưu raw chưa giải mã; Journal vẫn dừng bảo vệ dữ liệu:")
                || w.starts_with("PUBLISHED_DATA: Đã lưu raw chưa giải mã; Journal vẫn dừng bảo vệ dữ liệu:")
                || w.starts_with("PUBLISHED_ACK: Đã lưu bản ghi thử nghiệm toàn 0; Journal vẫn dừng bảo vệ dữ liệu:")
                || w.starts_with("PUBLISHED_DATA: Đã lưu bản ghi thử nghiệm toàn 0; Journal vẫn dừng bảo vệ dữ liệu:")))
        { return Ok(()); }
        let c = &snapshot.control;
        let start = (c.data_ready == 0 && c.total_count == 0)
            || (c.data_ready == 1 && (0..=1).contains(&c.total_count)
                && snapshot.buffer.as_ref().is_some_and(|b| b.buffer_index == 0
                    && b.raw_registers.len() == as200::map::RECORD_SIZE));
        if c.format_version != 1 || c.record_size != 240 || c.buffer_count != 2
            || c.pc_read_count != 0 || c.pc_read_done != 0 || !start
        { return Ok(()); }
        let history: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id WHERE s.source_key=$1 AND NOT COALESCE((i.session_id IS NULL AND i.processing_status IN ('SESSION_UNCONFIRMED','RAW_UNDECODED') AND (i.ingest_key LIKE 'raw-zero-test-v1:%' OR i.ingest_key LIKE 'raw-published-v1:%') AND i.record_seq=0 AND i.total_count=0 AND i.buffer_index=0 AND i.word_order=$3 AND i.raw_registers=$2), false)) OR EXISTS(SELECT 1 FROM measurements m JOIN plc_sources s ON s.id=m.source_id WHERE s.source_key=$1)",
        ).bind(&self.config.source_key).bind(serde_json::to_value(vec![0u16; as200::map::RECORD_SIZE]).map_err(|e| e.to_string())?)
            .bind(self.config.word_order.as_str()).fetch_one(pool).await.map_err(|e| e.to_string())?;
        if history { return Ok(()); }
        let mut journal = self.journal.lock().await;
        // published_gate serializes acquisition; also refuse concurrent retry work.
        if !journal.retries.is_empty() || !journal.manual_retries.is_empty() || journal.pending_ack()
            || !journal.deliveries.is_empty() || journal.pause != saved.pause
        { return Ok(()); }
        let mut next = journal.clone();
        next.pause = None;
        next.warnings.clear(); // Original diagnostic/control/raw remain in quarantine.
        Self::warn(&mut next, "Đã đối chiếu lỗi thử toàn 0 lúc khởi tạo: không có receipt hoặc ACK đã gửi; giữ nguyên raw, Session và checkpoint.".into());
        self.persist_published_transition(&mut journal, next)?;
        drop(journal);
        eprintln!("PLC_RECOVERY state=INITIAL_RAW_DIAGNOSTIC_RESOLVED endpoint={} total={} count=0 done=0 accepted=0 intent=none", self.config.address, c.total_count);
        self.publish_journal().await?;
        let mut status = self.state.status.write().await;
        status.state = "WAITING".into();
        status.last_error = None;
        status.ack_status = if self.config.read_only { "DISABLED" } else { "WAITING" };
        Ok(())
    }
}

impl WorkerState {
    pub(in crate::plc) fn require_sku_or_recovery(config: &SimulatorConfig, path: &std::path::Path) -> Result<(), String> {
        if config.operator_sku.is_some() { return Ok(()); }
        if path.exists() {
            let journal = Journal::load(path, config)?;
            if !journal.deliveries.is_empty() || journal.pending_ack() { return Ok(()); }
        }
        Err("Chưa nhập SKU QC. Vào Giám sát, nhập và lưu SKU trước khi kết nối PLC.".into())
    }

    // Caller holds settings_gate until the configuration write finishes.
    pub(in crate::plc) async fn verify_sku_change(
        &self, directory: &std::path::Path, config: &SimulatorConfig, pool: &PgPool,
    ) -> Result<Option<File>, String> {
        self.ensure_register_writes_stopped().await.map_err(|_| "Chưa thể đổi SKU: cần ngắt kết nối và đợi Worker/ACK dừng hoàn toàn.".to_string())?;
        let path = config.journal_file(&directory.join("as200_config.toml"));
        if !path.exists() {
            super::super::connection_settings::prepare_collection(directory, config, pool).await?;
            // Startup may accept a first published record, but changing SKU may
            // not relabel data already waiting on the PLC. Do not inspect stale RAM.
            let snapshot = config.plc_reader().poll_published_record().await.map_err(|e| e.to_string())?;
            let c = &snapshot.control;
            if c.total_count != 0 || c.pc_read_count != 0 || c.pc_read_done != 0 || c.data_ready != 0 {
                return Err("Chưa thể đổi SKU: PLC đã có dữ liệu hoặc checkpoint cần đối chiếu. Giữ SKU hiện có để tiếp nhận bản ghi chờ.".into());
            }
            return Ok(None);
        }
        let lock = OpenOptions::new().read(true).write(true).open(path.with_extension("lock"))
            .map_err(|_| "Không xác minh được khóa Journal; chưa đổi SKU.".to_string())?;
        lock.try_lock().map_err(|_| "Journal đang được sử dụng; chưa đổi SKU.".to_string())?;
        let journal = Journal::load(&path, config)?;
        let accepted = journal.published_ack.as_ref().ok_or("Journal thiếu ACK progress.")?.accepted;
        let prefix = crate::services::inbox_service::raw_ack_prefix(journal.session_id.as_deref().ok_or("Thiếu Session.")?);
        let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id WHERE s.source_key=$1 AND (i.processing_status<>'PROCESSED' OR i.measurement_id IS NULL OR i.processed_at IS NULL) AND NOT COALESCE((i.session_id IS NULL AND i.ingest_key=$2 || i.total_count::text AND i.total_count BETWEEN 1 AND $3 AND i.processing_status='SESSION_UNCONFIRMED' AND i.measurement_id IS NULL AND i.processed_at IS NULL),false))")
            .bind(&config.source_key).bind(prefix).bind(i64::from(accepted)).fetch_one(pool).await
            .map_err(|_| "Không đối chiếu được Inbox; chưa đổi SKU.".to_string())?;
        if pending { return Err("Chưa thể đổi SKU: nguồn PLC còn Inbox hoặc raw chưa được đối chiếu ACK.".into()); }
        if journal.pause.is_some() || journal.pending_ack() || !journal.deliveries.is_empty()
            || !journal.retries.is_empty() || !journal.quarantine.is_empty()
            || journal.manual_retries.iter().any(|r| matches!(r.state.as_str(), "PENDING" | "WAITING"))
            || journal.last_total.is_some_and(|total| total != accepted)
        {
            return Err("Chưa thể đổi SKU: Journal còn dữ liệu, ACK chưa hoàn tất hoặc trạng thái cần đối chiếu. Kết nối lại để hoàn tất SKU cũ trước.".into());
        }
        verify_history(config, &journal, pool).await?;
        let mut reader = config.plc_reader();
        let first = reader.poll_published_record().await.map_err(|e| format!("Không đọc được PLC để xác minh đổi SKU: {e}"))?;
        let second = reader.poll_published_record().await.map_err(|e| format!("Không đọc lại được PLC để xác minh đổi SKU: {e}"))?;
        for snapshot in [&first, &second] {
            let c = &snapshot.control;
            if !snapshot.diagnostics.is_empty() || snapshot.buffer.is_some() || c.data_ready != 0
                || c.pc_read_done != 0 || c.pc_read_count != accepted || c.total_count != accepted
            {
                return Err("Chưa thể đổi SKU: PLC còn bản ghi chờ hoặc counter/ACK không khớp Journal. Không tự reset hay bỏ dữ liệu.".into());
            }
        }
        // Both stable snapshots already prove an idle checkpoint. Old words in
        // the published region have no meaning while DataReady=0.
        Ok(Some(lock))
    }
}
