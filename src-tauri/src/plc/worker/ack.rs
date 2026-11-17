//! Published Record handshake; persisted phases precede all PLC writes.
use super::*;

impl Engine {
    pub(super) async fn verify_published_write_snapshot(
        &self,
        intent: &PublishedAckIntent,
        expected_count: i32,
    ) -> Result<PublishedRecordSnapshot, String> {
        let snapshot = self.read_snapshot().await?;
        let previous = self.journal.lock().await.last_total;
        if previous.is_some_and(|total| snapshot.control.total_count < total) {
            return self
                .pause_published_ack_snapshot(
                    &snapshot,
                    "RESET: TotalCount giảm trong lần đọc lại trước ACK; giữ phiên/intent.".into(),
                )
                .await;
        }
        self.observe_published_total(snapshot.control.total_count)
            .await?;
        if let Err(error) = intent.check_before_write(&snapshot, expected_count) {
            if snapshot.control.total_count > intent.target {
                return Err(error); // Policy not proved by Excel; do not lower the target restriction.
            }
            return self.pause_published_ack_snapshot(&snapshot, error).await;
        }
        if self.state.stopping.load(Ordering::SeqCst) || !self.state.fresh_health().await {
            return Err("Chờ heartbeat PLC hoạt động ổn định; chưa ghi ACK.".into());
        }
        Ok(snapshot)
    }

    pub(super) async fn pause_published_ack_snapshot<T>(
        &self,
        snapshot: &PublishedRecordSnapshot,
        error: String,
    ) -> Result<T, String> {
        let mut journal = self.journal.lock().await;
        let mut next = journal.clone();
        next.pause = Some(
            if snapshot.control.total_count
                < journal.published_ack.as_ref().map_or(0, |p| p.accepted)
                || journal
                    .last_total
                    .is_some_and(|total| snapshot.control.total_count < total)
            {
                "RESET"
            } else {
                "PUBLISHED_ACK_RECONCILIATION"
            }
            .into(),
        );
        Self::warn(&mut next, format!("{error} control={:?}", snapshot.control));
        if let Some(buffer) = &snapshot.buffer {
            if next.quarantine.len() < self.config.max_queue {
                next.quarantine.push(QuarantinedRecord {
                    control: Some(snapshot.control.clone()),
                    source_key: self.config.source_key.clone(),
                    session_id: next.session_id.clone(),
                    address: self.config.address.clone(),
                    unit_id: self.config.unit_id,
                    modbus_address_offset: self.config.modbus_address_offset,
                    word_order: self.config.word_order,
                    buffer_index: buffer.buffer_index,
                    record_seq: decode::dint32(
                        &buffer.raw_registers,
                        super::as200::map::record::SEQUENCE,
                        self.config.reader().word_order,
                    )
                    .ok(),
                    total_count: snapshot.control.total_count,
                    received_at: Utc::now(),
                    raw_registers: buffer.raw_registers.clone(),
                    error: error.clone(),
                });
            }
        }
        self.persist_published_transition(&mut journal, next)?;
        drop(journal);
        self.publish_journal().await?;
        Err(error)
    }

    pub(super) async fn published_ack_step(
        &self,
        snapshot: PublishedRecordSnapshot,
        pool: &PgPool,
    ) -> Result<(), String> {
        if self.config.read_only
            || !self.config.ack_enabled
            || !simulator_config::ack_scope_allows(&self.config)
        {
            return Err("ACK bị khóa: cấu hình chỉ đọc hoặc chưa được xác minh.".into());
        }
        let saved = self.journal.lock().await.clone();
        saved.validate(&self.config)?;
        if saved.pause.is_some() {
            return Err("Journal v4 đang pause; không ghi ACK.".into());
        }
        let progress = saved
            .published_ack
            .as_ref()
            .ok_or("Thiếu ACK progress v4.")?;
        let c = &snapshot.control;
        if c.format_version != 1
            || c.record_size != 240
            || c.buffer_count != 2
            || c.total_count < progress.accepted
            || saved
                .last_total
                .is_some_and(|previous| c.total_count < previous)
            || c.pc_read_count < 0
            || c.pc_read_count > c.total_count
            || !matches!(c.pc_read_done, 0 | 1)
            || !matches!(c.data_ready, 0 | 1)
        {
            return self.pause_published_ack_snapshot(&snapshot, format!("RESET/CONFLICT: control không khớp lịch sử v4; total={}, count={}, done={}, accepted={}. Không tự đổi phiên.", c.total_count, c.pc_read_count, c.pc_read_done, progress.accepted)).await;
        }
        if !snapshot.diagnostics.is_empty() {
            return self
                .pause_published_ack_snapshot(
                    &snapshot,
                    format!("PUBLISHED_DIAGNOSTIC: {:?}", snapshot.diagnostics),
                )
                .await;
        }
        // Remember increases observed while an intent is pending too, so a later
        // decrease cannot become invisible merely because no record was captured.
        self.observe_published_total(c.total_count).await?;
        let pending = progress
            .intent
            .as_ref()
            .filter(|i| i.phase != PublishedAckPhase::AckConfirmed);
        let Some(intent) = pending else {
            if c.pc_read_count != progress.accepted || c.pc_read_done != 0 {
                return Err(
                    "PLC có checkpoint/Done không thuộc intent v4; không tự nhận là đã ACK.".into(),
                );
            }
            if c.data_ready == 0 {
                if c.total_count != progress.accepted {
                    return Err("DataReady=0 nhưng TotalCount chưa được đối chiếu với accepted; không xác nhận ACK.".into());
                }
                self.capture_published_snapshot(snapshot).await?;
                self.state.status.write().await.ack_status = if progress.accepted > 0 {
                    "ACKNOWLEDGED"
                } else {
                    "WAITING"
                };
                return Ok(());
            }
            if let Some(record) = snapshot.buffer.as_ref().and_then(|b| b.record.as_ref()) {
                if record.record_seq <= progress.accepted {
                    return Err(
                        "PLC công bố record đã ACK; không lưu lại hoặc tự phát Done lần nữa."
                            .into(),
                    );
                }
            }
            self.capture_published_snapshot(snapshot.clone()).await?;
            self.flush(pool).await?;
            let delivery = self
                .journal
                .lock()
                .await
                .deliveries
                .iter()
                .find(|d| {
                    snapshot
                        .buffer
                        .as_ref()
                        .and_then(|b| b.record.as_ref())
                        .is_some_and(|r| d.input.record_seq == r.record_seq)
                })
                .cloned()
                .ok_or("Thiếu receipt Bản ghi 1 vừa capture.")?;
            recovery::processed_receipt(&self.config, &delivery, pool).await?;
            let prepared = PublishedAckIntent {
                target: delivery.input.record_seq, // Never substitute TotalCount.
                inbox_id: delivery.inbox_id.ok_or("Receipt chưa có Inbox ID.")?,
                raw_registers: delivery.input.raw_registers.clone(),
                phase: PublishedAckPhase::Prepared,
                observed_done_high: false,
            };
            let mut journal = self.journal.lock().await;
            let mut next = journal.clone();
            next.published_ack
                .as_mut()
                .ok_or("Thiếu ACK progress v4.")?
                .intent = Some(prepared);
            self.persist_published_transition(&mut journal, next)?;
            self.state.status.write().await.ack_status = "PENDING";
            return Ok(()); // A later iteration re-reads before any write.
        };

        let delivery = saved
            .deliveries
            .iter()
            .find(|d| d.input.record_seq == intent.target)
            .ok_or("Intent Bản ghi 1 thiếu receipt; không ghi PLC.")?;
        recovery::processed_receipt(&self.config, delivery, pool).await?;
        if (matches!(
            intent.phase,
            PublishedAckPhase::CountVerified | PublishedAckPhase::DoneWriteUncertain
        ) && c.pc_read_count != intent.target)
            || snapshot.buffer.as_ref().is_some_and(|b| {
                b.record.as_ref().is_some_and(|r| {
                    r.record_seq == intent.target && b.raw_registers != intent.raw_registers
                })
            })
        {
            return self.pause_published_ack_snapshot(&snapshot,
                "CONFLICT: checkpoint đã xác minh bị thay đổi hoặc raw cùng RecordSeq khác intent; không tự phục hồi bằng counter mới.".into()).await;
        }
        let mut updated = intent.clone();
        if matches!(
            intent.phase,
            PublishedAckPhase::Prepared | PublishedAckPhase::CountVerified
        ) && (self.state.stopping.load(Ordering::SeqCst) || !self.state.fresh_health().await)
        {
            let mut status = self.state.status.write().await;
            if status.ack_status != "RECONCILIATION_REQUIRED" {
                status.ack_status = "WAITING";
            }
            return Ok(()); // Heartbeat warmup/stall is not evidence of a failed write.
        }
        match intent.phase {
            PublishedAckPhase::Prepared => {
                self.verify_published_write_snapshot(intent, progress.accepted)
                    .await?;
                let writer = super::as200::ack::AckWriter::new(
                    self.config.reader(),
                    &self.config.mode,
                    self.config.ack_enabled,
                    self.config.ack_protocol.as_deref(),
                    self.config.allow_plc_connection,
                )?;
                updated.phase = PublishedAckPhase::CountWriteUncertain;
                self.update_published_intent(updated.clone()).await?;
                writer.count(intent.target).await?;
                // Echo is insufficient; only a matching read-back verifies Count.
                let checked = self
                    .verify_published_write_snapshot(intent, intent.target)
                    .await?;
                if updated.same_published_record(&checked) {
                    updated.phase = PublishedAckPhase::CountVerified;
                    self.update_published_intent(updated).await?;
                }
            }
            PublishedAckPhase::CountWriteUncertain => {
                if c.pc_read_count != intent.target
                    || c.pc_read_done != 0
                    || !intent.same_published_record(&snapshot)
                {
                    return Err("COUNT_WRITE_UNCERTAIN: chưa đối chiếu được tác dụng Count; chỉ quan sát, không gửi lại Count/Done.".into());
                }
                updated.phase = PublishedAckPhase::CountVerified;
                self.update_published_intent(updated).await?;
            }
            PublishedAckPhase::CountVerified => {
                self.verify_published_write_snapshot(intent, intent.target)
                    .await?;
                let writer = super::as200::ack::AckWriter::new(
                    self.config.reader(),
                    &self.config.mode,
                    self.config.ack_enabled,
                    self.config.ack_protocol.as_deref(),
                    self.config.allow_plc_connection,
                )?;
                let stream = writer.prepare_done().await?;
                // Socket preparation may take time; verify again before marking/writing Done.
                self.verify_published_write_snapshot(intent, intent.target)
                    .await?;
                updated.phase = PublishedAckPhase::DoneWriteUncertain;
                self.update_published_intent(updated.clone()).await?;
                writer.done_prepared(stream).await?;
                // Try to observe the high level promptly, but never infer success
                // if the pulse has already disappeared or this read fails.
                if let Ok(control) = As200Reader::new(self.config.reader()).read_control().await {
                    if control.pc_read_count == intent.target
                        && control.pc_read_done == 1
                        && control.total_count >= intent.target
                        && control.total_count >= c.total_count
                    {
                        self.observe_published_total(control.total_count).await?;
                        updated.observed_done_high = true;
                        self.update_published_intent(updated).await?;
                    }
                }
            }
            PublishedAckPhase::DoneWriteUncertain => {
                if c.pc_read_count == intent.target
                    && c.pc_read_done == 1
                    && intent.same_published_record(&snapshot)
                {
                    updated.observed_done_high = true;
                    self.update_published_intent(updated).await?;
                } else if intent.confirmed_by(&snapshot) {
                    self.confirm_published_ack(intent, delivery, pool, &snapshot)
                        .await?;
                    self.state.status.write().await.ack_status = "ACKNOWLEDGED";
                    return Ok(());
                } else {
                    return Err("DONE_WRITE_UNCERTAIN: chưa đủ bằng chứng PLC xử lý Done; có thể đã bỏ lỡ xung. Giữ intent, không tự kết luận thành công/thất bại hoặc gửi lại.".into());
                }
            }
            PublishedAckPhase::AckConfirmed => unreachable!("confirmed intent was filtered"),
        }
        self.state.status.write().await.ack_status = "WAITING";
        Ok(())
    }

    pub(super) async fn update_published_intent(
        &self,
        intent: PublishedAckIntent,
    ) -> Result<(), String> {
        let mut journal = self.journal.lock().await;
        if journal.pause.is_some() {
            return Err("Journal đang pause; không chuyển pha ACK.".into());
        }
        let mut next = journal.clone();
        next.published_ack
            .as_mut()
            .ok_or("Thiếu ACK progress v4.")?
            .intent = Some(intent);
        self.persist_published_transition(&mut journal, next)
    }

    pub(super) async fn observe_published_total(&self, total: i32) -> Result<(), String> {
        let mut journal = self.journal.lock().await;
        if journal.last_total.is_none_or(|previous| total > previous) {
            let mut next = journal.clone();
            next.last_total = Some(total);
            self.persist_published_transition(&mut journal, next)?;
        }
        Ok(())
    }

    pub(super) async fn confirm_published_ack(
        &self,
        intent: &PublishedAckIntent,
        delivery: &Delivery,
        pool: &PgPool,
        snapshot: &PublishedRecordSnapshot,
    ) -> Result<(), String> {
        let row = recovery::processed_receipt(&self.config, delivery, pool).await?;
        let mut journal = self.journal.lock().await;
        let next = self.published_confirmation_candidate(&journal, intent, delivery, snapshot)?;
        self.persist_published_transition(&mut journal, next)?;
        drop(journal);
        self.row_status(&row).await
    }

    pub(super) fn published_confirmation_candidate(
        &self,
        journal: &Journal,
        intent: &PublishedAckIntent,
        delivery: &Delivery,
        snapshot: &PublishedRecordSnapshot,
    ) -> Result<Journal, String> {
        if journal.pause.is_some()
            || journal
                .published_ack
                .as_ref()
                .and_then(|p| p.intent.as_ref())
                != Some(intent)
            || !intent.confirmed_by(snapshot)
            || delivery.inbox_id != Some(intent.inbox_id)
            || delivery.input.record_seq != intent.target
            || delivery.input.raw_registers != intent.raw_registers
        {
            return Err("Chưa đủ bằng chứng PLC/receipt để compact Journal ACK.".into());
        }
        let mut next = journal.clone();
        let progress = next
            .published_ack
            .as_mut()
            .ok_or("Thiếu ACK progress v4.")?;
        if progress.accepted.checked_add(1) != Some(intent.target) {
            return Err(
                "ACK Bản ghi 1 có sequence gap khi xác nhận; không tăng checkpoint.".into(),
            );
        }
        let mut confirmed = intent.clone();
        confirmed.phase = PublishedAckPhase::AckConfirmed;
        progress.accepted = intent.target;
        progress.confirmed_receipt = Some(delivery.clone());
        progress.intent = Some(confirmed);
        next.last_total = Some(snapshot.control.total_count);
        // The only removal is the exact PROCESSED receipt just confirmed by PLC.
        // Keep an anchor in progress and all unconfirmed/unprocessed deliveries.
        next.deliveries
            .retain(|d| d.input.record_seq != intent.target);
        next.warnings.retain(|w| !w.starts_with("PUBLISHED_ACK:"));
        next.validate(&self.config)?;
        Ok(next)
    }
}
