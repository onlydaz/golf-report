//! Validate and persist only the single published region.
use super::*;

const RAW_PENDING: &str = "RAW_UNDECODED_PENDING:";
const RAW_SAVED: &str = "RAW_UNDECODED_SAVED:";
const LEGACY_ZERO_PENDING: &str = "RAW_ZERO_TEST_PENDING:";
const LEGACY_ZERO_SAVED: &str = "RAW_ZERO_TEST_SAVED:";
const LEGACY_INVALID_PENDING: &str = "RAW_INVALID_PENDING:";
const LEGACY_INVALID_SAVED: &str = "RAW_INVALID_SAVED:";

impl Engine {
    // A stable publication is identified by the next actual PLC counter, even
    // when its payload cannot be decoded. Keep the raw RecordSeq unchanged.
    pub(super) async fn capture_counter_raw(
        &self, snapshot: &PublishedRecordSnapshot,
    ) -> Result<(), String> {
        let mut journal = self.journal.lock().await;
        journal.validate(&self.config)?;
        if journal.pause.is_some() || journal.pending_ack() {
            return Err("Journal/ACK còn cần đối chiếu; không tạo delivery mới.".into());
        }
        let c = &snapshot.control;
        let accepted = journal.published_ack.as_ref().ok_or("Thiếu ACK progress.")?.accepted;
        let buffer = snapshot.buffer.as_ref().filter(|b| b.buffer_index == 0
            && b.raw_registers.len() == as200::map::RECORD_SIZE)
            .ok_or("Thiếu 240 WORD Bản ghi 1.")?;
        if c.format_version != 1 || c.record_size != 240 || c.buffer_count != 2
            || c.data_ready != 1 || accepted.checked_add(1) != Some(c.total_count)
            || c.pc_read_count != accepted || c.pc_read_done != 0
            || journal.last_total.is_some_and(|n| c.total_count < n)
        {
            return Err("Chưa xác định được bản ghi mới từ TotalCount/PC_ReadCount/DataReady; không ACK.".into());
        }
        if let Some(existing) = journal.deliveries.iter().find(|d| d.ack_target() == c.total_count) {
            if existing.input.raw_registers != buffer.raw_registers {
                return Err("CONFLICT: raw thay đổi trong cùng counter PLC; giữ receipt, không ACK.".into());
            }
            return Ok(());
        }
        if !journal.deliveries.is_empty() || journal.deliveries.len() >= self.config.max_queue {
            return Err("Journal còn bản ghi khác chưa đối chiếu; không bỏ qua dữ liệu.".into());
        }
        let session = journal.session_id.as_deref().ok_or("Thiếu Session đã xác nhận.")?;
        let input = InboxInput {
            source_id: 0,
            // No valid measurement identity exists in the payload. The key binds
            // the real operating session and control counter without inventing RecordSeq.
            confirmed_session_id: None,
            ingest_key: format!("{}{}", crate::services::inbox_service::raw_ack_prefix(session), c.total_count),
            record_seq: decode::dint32(&buffer.raw_registers, as200::map::record::SEQUENCE,
                self.config.reader().word_order).map_err(|e| e.to_string())?,
            total_count: Some(i64::from(c.total_count)), buffer_index: 0,
            raw_registers: buffer.raw_registers.clone(), word_order: self.config.word_order,
            received_at: Utc::now(),
        };
        if input.ingest_key.len() > 200 {
            return Err("Session quá dài để lưu định danh raw vào Inbox; không ACK.".into());
        }
        let mut next = journal.clone();
        next.deliveries.push(Delivery { input, inbox_id: None, terminal: false });
        next.last_total = Some(c.total_count);
        self.persist_published_transition(&mut journal, next)
    }

    // Without a new counter, repeated raw snapshots are one observation, not
    // distinct sticks. Preserve them without manufacturing an ACK identity.
    pub(super) async fn capture_invalid_snapshot(
        &self, snapshot: &PublishedRecordSnapshot, pool: &PgPool,
    ) -> Result<(), String> {
        self.archive_unmapped_snapshot(snapshot, pool).await?;
        let journal = self.journal.lock().await.clone();
        if let Some(pause) = &journal.pause {
            return Err(format!("Đã lưu raw chưa giải mã; Journal giữ trạng thái bảo vệ: {pause}."));
        }
        let accepted = journal.published_ack.as_ref().ok_or("Thiếu ACK progress.")?.accepted;
        let c = &snapshot.control;
        if c.format_version != 1 || c.record_size != 240 || c.buffer_count != 2
            || c.pc_read_count < 0 || c.pc_read_count > c.total_count
            || journal.pending_ack() || !journal.deliveries.is_empty()
            || c.total_count < accepted || journal.last_total.is_some_and(|n| c.total_count < n)
            || c.pc_read_count != accepted || c.pc_read_done != 0
        {
            return self.pause_published_ack_snapshot(snapshot,
                "PUBLISHED_DIAGNOSTIC: raw chưa hợp lệ trong khi có checkpoint/receipt/ACK cần đối chiếu; không ghi PLC.".into()).await;
        }
        self.observe_published_total(c.total_count).await?;
        let mut status = self.state.status.write().await;
        status.state = "RAW_UNDECODED".into();
        status.ack_status = if self.config.read_only { "DISABLED" } else { "BLOCKED_UNIDENTIFIED_RAW" };
        status.last_error = None;
        status.message = format!(
            "Đã lưu nguyên trạng 240 WORD; chưa tạo measurement. Chưa đủ điều kiện ACK: TotalCount={}, PC_ReadCount={}, accepted={}; cần counter kế tiếp và handshake khớp Journal.",
            c.total_count, c.pc_read_count, accepted
        );
        Ok(())
    }

    async fn archive_unmapped_snapshot(
        &self, snapshot: &PublishedRecordSnapshot, pool: &PgPool,
    ) -> Result<(), String> {
        let buffer = snapshot.buffer.as_ref().filter(|buffer| snapshot.control.data_ready == 1
            && buffer.buffer_index == 0 && buffer.raw_registers.len() == as200::map::RECORD_SIZE)
            .ok_or("Raw capture cần DataReady=1 và đúng 240 WORD tại Bản ghi 1.")?;
        let c = &snapshot.control;
        let pending_prefix = RAW_PENDING;
        let capture = {
            let mut journal = self.journal.lock().await;
            journal.validate(&self.config)?;
            let existing = journal.quarantine.iter().find(|q| {
                (q.error.starts_with(pending_prefix) || q.error.starts_with(RAW_SAVED)
                    || q.error.starts_with(LEGACY_ZERO_PENDING) || q.error.starts_with(LEGACY_ZERO_SAVED)
                    || q.error.starts_with(LEGACY_INVALID_PENDING) || q.error.starts_with(LEGACY_INVALID_SAVED))
                    && q.source_key == journal.source_key && q.session_id == journal.session_id
                    && q.address == self.config.address && q.unit_id == self.config.unit_id
                    && q.word_order == self.config.word_order && q.modbus_address_offset == self.config.modbus_address_offset
                    && q.buffer_index == 0 && q.raw_registers == buffer.raw_registers
                    && q.control.as_ref().is_some_and(|control| control.total_count == c.total_count
                        && control.pc_read_count == c.pc_read_count && control.pc_read_done == c.pc_read_done)
            }).cloned();
            if let Some(capture) = existing { capture } else {
                if journal.quarantine.len() >= self.config.max_queue {
                    return Err("Vùng lưu raw cách ly đã đầy; chưa lưu snapshot mới, không gửi ACK. Giữ Journal để kỹ thuật đối chiếu.".into());
                }
                let raw = buffer.raw_registers.clone();
                let capture = QuarantinedRecord {
                    control: Some(c.clone()), source_key: journal.source_key.clone(), session_id: journal.session_id.clone(),
                    address: self.config.address.clone(), unit_id: self.config.unit_id,
                    modbus_address_offset: self.config.modbus_address_offset, word_order: self.config.word_order,
                    buffer_index: 0,
                    record_seq: Some(decode::dint32(&raw, as200::map::record::SEQUENCE, self.config.reader().word_order)
                        .map_err(|e| e.to_string())?),
                    total_count: c.total_count, received_at: Utc::now(), raw_registers: raw,
                    error: format!("{pending_prefix} raw chính thức chưa giải mã; diagnostics={:?}", snapshot.diagnostics),
                };
                let mut next = journal.clone();
                next.quarantine.push(capture.clone());
                self.persist_published_transition(&mut journal, next)?;
                        eprintln!("PLC_RAW state=JOURNALED endpoint={} total={} count={} done={} seq={:?} zero={} ack_eligible=false", self.config.address, c.total_count, c.pc_read_count, c.pc_read_done, capture.record_seq, snapshot.is_zero_test());
                capture
            }
        };
        self.save_unmapped_capture(&capture, pool).await.map_err(|error| format!(
            "Raw được giữ trong Journal nhưng chưa xác nhận lưu PostgreSQL: {error}. Không tạo measurement hoặc gửi ACK."))
    }

    pub(super) async fn flush_unmapped_captures(&self, pool: &PgPool) -> Result<(), String> {
        let pending: Vec<_> = self.journal.lock().await.quarantine.iter()
            .filter(|q| q.error.starts_with(RAW_PENDING)
                || q.error.starts_with(LEGACY_ZERO_PENDING)
                || q.error.starts_with(LEGACY_INVALID_PENDING)).cloned().collect();
        for capture in pending {
            self.save_unmapped_capture(&capture, pool).await.map_err(|error| format!(
                "Raw còn chờ lưu PostgreSQL: {error}. Giữ Journal để thử lại; không gửi ACK."))?;
        }
        Ok(())
    }

    async fn save_unmapped_capture(&self, capture: &QuarantinedRecord, pool: &PgPool) -> Result<(), String> {
        let legacy_zero = capture.error.starts_with(LEGACY_ZERO_PENDING)
            || capture.error.starts_with(LEGACY_ZERO_SAVED);
        let legacy = legacy_zero
            || capture.error.starts_with(LEGACY_INVALID_PENDING)
            || capture.error.starts_with(LEGACY_INVALID_SAVED);
        let zero = capture.raw_registers.iter().all(|word| *word == 0);
        let pending_prefix = if legacy {
            if legacy_zero { LEGACY_ZERO_PENDING } else { LEGACY_INVALID_PENDING }
        } else { RAW_PENDING };
        let saved_prefix = if legacy {
            if legacy_zero { LEGACY_ZERO_SAVED } else { LEGACY_INVALID_SAVED }
        } else { RAW_SAVED };
        if capture.source_key != self.config.source_key || capture.session_id != self.config.confirmed_session_id
            || capture.address != self.config.address || capture.unit_id != self.config.unit_id
            || capture.word_order != self.config.word_order || capture.modbus_address_offset != self.config.modbus_address_offset
            || capture.buffer_index != 0 || capture.raw_registers.len() != as200::map::RECORD_SIZE
            || (zero && capture.raw_registers.iter().any(|w| *w != 0))
        { return Err("Raw cách ly không khớp nguồn/phiên hoặc 240 WORD; giữ nguyên Journal để đối chiếu.".into()); }
        let control = capture.control.as_ref().ok_or("Raw cách ly thiếu control snapshot.")?;
        if control.data_ready != 1 || control.total_count != capture.total_count {
            return Err("Raw cách ly không khớp control đã lưu; không tự sửa dữ liệu.".into());
        }
        if decode::dint32(&capture.raw_registers, as200::map::record::SEQUENCE, self.config.reader().word_order)
            .ok() != capture.record_seq {
            return Err("RecordSeq không khớp raw đã giữ; không tự sửa định danh bản ghi.".into());
        }
        if capture.error.starts_with(pending_prefix) {
            self.state.status.write().await.raw_capture = None;
        }
        if zero && capture.error.starts_with(saved_prefix) && self.state.status.read().await.raw_capture.as_ref()
            .is_some_and(|saved| saved.received_at == capture.received_at && saved.total_count == capture.total_count)
        { return Ok(()); }
        let session = capture.session_id.as_deref().ok_or("Raw cách ly thiếu ngữ cảnh Session.")?;
        let key = if legacy_zero {
            format!("{}{}:{session}:{}:{}:{}:{}", crate::services::inbox_service::LEGACY_RAW_ZERO_KEY,
                session.len(), control.total_count, control.pc_read_count, control.pc_read_done, capture.word_order.as_str())
        } else if legacy {
            format!("{}{}:{}", crate::services::inbox_service::LEGACY_RAW_INVALID_KEY,
                capture.received_at.timestamp_nanos_opt().ok_or("Thời điểm raw capture không hợp lệ.")?,
                capture.record_seq.ok_or("Raw thiếu RecordSeq gốc.")?)
        } else {
            format!("{}{}:{}:{}:{}:{}:{}",
                crate::services::inbox_service::RAW_UNDECODED_KEY,
                capture.received_at.timestamp_nanos_opt().ok_or("Thời điểm raw capture không hợp lệ.")?,
                session.len(), control.total_count, control.pc_read_count, control.pc_read_done,
                capture.word_order.as_str())
        };
        let source = SourceRepository::upsert(pool, &capture.source_key, None).await?;
        let input = InboxInput {
            source_id: source.id,
            // No confirmed measurement identity exists in these words. Keep the
            // actual operating session in Journal/key; never alter the PLC session.
            confirmed_session_id: None, ingest_key: key,
            record_seq: capture.record_seq.ok_or("Raw cách ly thiếu giá trị RecordSeq gốc.")?,
            total_count: Some(i64::from(control.total_count)), buffer_index: 0,
            raw_registers: capture.raw_registers.clone(), word_order: capture.word_order, received_at: capture.received_at,
        };
        let row = if capture.error.starts_with(saved_prefix) {
            let prefix = format!("{saved_prefix} inbox_id=");
            let id = capture.error.strip_prefix(&prefix).and_then(|s| s.split(';').next())
                .and_then(|s| s.parse::<i64>().ok()).ok_or("Raw cách ly thiếu Inbox ID đã xác nhận; cần đối chiếu Journal.")?;
            InboxRepository::find_optional(pool, id).await?
                .ok_or("LOCAL_HISTORY_MISSING: Inbox raw đã lưu không còn trong PostgreSQL; giữ Journal, không tự tạo bản thay thế.")?
        } else {
            match InboxRepository::existing_delivery(pool, source.id, &input).await? {
                Some(row) => row,
                None => InboxService::receive(pool, &input).await?,
            }
        };
        // Commit may have succeeded before a lost response. Read back the same
        // deterministic key, validate the payload and never create a replacement.
        let row = InboxRepository::find(pool, row.id).await?;
        if row.source_id != source.id || row.ingest_key != input.ingest_key || row.session_id.is_some()
            || row.record_seq != input.record_seq || row.total_count != input.total_count || row.buffer_index != 0
            || row.word_order != input.word_order.as_str()
            || row.raw_registers != serde_json::to_value(&input.raw_registers).map_err(|e| e.to_string())?
            || !matches!(row.processing_status.as_str(), "SESSION_UNCONFIRMED" | "RAW_UNDECODED")
            || row.measurement_id.is_some() || row.processed_at.is_some()
        { return Err("CONFLICT: Inbox raw cách ly không khớp; không ghi đè hoặc ACK.".into()); }
        {
            let mut journal = self.journal.lock().await;
            let mut next = journal.clone();
            if let Some(q) = next.quarantine.iter_mut().find(|q| q.received_at == capture.received_at
                && q.error.starts_with(pending_prefix) && q.raw_registers == capture.raw_registers)
            {
                q.error = format!("{saved_prefix} inbox_id={}; raw chính thức, chưa giải mã thành measurement hoặc receipt ACK.", row.id);
                self.persist_published_transition(&mut journal, next)?;
                eprintln!("PLC_RAW state=STORED inbox={} endpoint={} total={} measurement=false ack_eligible=false", row.id, self.config.address, capture.total_count);
            }
        }
        self.state.status.write().await.raw_capture = Some(RawCaptureStatus {
            inbox_id: row.id, received_at: capture.received_at, total_count: capture.total_count,
            kind: if zero { "ZERO" } else { "UNDECODED" },
            raw_registers: capture.raw_registers.clone(),
        });
        Ok(())
    }

    pub(super) fn published_delivery(
        &self,
        journal: &Journal,
        snapshot: &PublishedRecordSnapshot,
    ) -> Result<Option<(InboxInput, LatestRecord)>, String> {
        journal.validate(&self.config)?;
        let progress = journal
            .published_ack
            .as_ref()
            .ok_or("Thiếu ACK progress v4.")?;
        if progress
            .intent
            .as_ref()
            .is_some_and(|i| i.phase != PublishedAckPhase::AckConfirmed)
        {
            return Err("ACK còn intent; chỉ quan sát handshake, chưa capture record khác.".into());
        }
        let accepted = progress.accepted;
        let control = &snapshot.control;
        if control.format_version != 1
            || control.record_size != 240
            || control.buffer_count != 2
            || control.total_count < 0
            || !matches!(control.data_ready, 0 | 1)
            || control.pc_read_count != accepted
            || control.pc_read_done != 0
        {
            return Err("PUBLISHED_HANDSHAKE: cấu trúc hoặc checkpoint PLC không khớp Journal; chưa cho phép ACK.".into());
        }
        if journal
            .last_total
            .is_some_and(|previous| control.total_count < previous)
        {
            return Err(
                "RESET: TotalCount giảm; giữ nguyên phiên và Journal, cần đối chiếu.".into(),
            );
        }
        if !snapshot.diagnostics.is_empty() {
            return Err(format!("PUBLISHED_DIAGNOSTIC: {:?}", snapshot.diagnostics));
        }
        let highest = journal
            .deliveries
            .iter()
            .map(Delivery::ack_target)
            .max()
            .unwrap_or(accepted)
            .max(accepted);
        if control.data_ready == 0 {
            if snapshot.buffer.is_some() || control.total_count != highest {
                return Err("PUBLISHED_DIAGNOSTIC: DataReady=0 nhưng snapshot/lịch sử chưa chứng minh đầy đủ dữ liệu; không tự bỏ qua.".into());
            }
            return Ok(None);
        }
        let buffer = snapshot
            .buffer
            .as_ref()
            .ok_or("PUBLISHED_DIAGNOSTIC: DataReady=1 nhưng thiếu Bản ghi 1.")?;
        if buffer.buffer_index != 0 {
            return Err("PUBLISHED_DIAGNOSTIC: chỉ tiếp nhận vùng Bản ghi 1 (index 0).".into());
        }
        // Decode raw again at the capture boundary; never trust a record detached from its raw.
        let record = decode::record(&buffer.raw_registers, 0, self.config.reader().word_order)
            .map_err(|e| e.to_string())?;
        record.validate_metadata(Some(i64::from(control.total_count)))?;
        record.validate_workflow()?;
        if !record.complete() || record.record_seq <= 0 || buffer.record.as_ref() != Some(&record) {
            return Err(
                "PUBLISHED_DIAGNOSTIC: record/raw/Status chưa hợp lệ hoặc chưa hoàn tất.".into(),
            );
        }
        if record.record_seq <= accepted {
            return Err("PLC đang công bố sequence đã ACK; đối chiếu Neon, không tạo delivery hoặc gửi ACK lại.".into());
        }
        if let Some(existing) = journal
            .deliveries
            .iter()
            .find(|d| d.ack_target() == record.record_seq)
        {
            if existing.input.raw_registers != buffer.raw_registers
                || existing.input.word_order != self.config.word_order
                || existing.input.buffer_index != 0
            {
                return Err("CONFLICT: cùng RecordSeq nhưng raw/WordOrder/vùng công bố thay đổi; không tạo phép đo mới.".into());
            }
            return Ok(None); // Includes reappearing old receipts; never recreate their delivery key.
        }
        if highest.checked_add(1) != Some(record.record_seq) {
            return Err(format!(
                "SEQUENCE_GAP: đã tiếp nhận đến {highest}, PLC công bố {}; giữ dữ liệu để đối chiếu, không tăng checkpoint ACK.",
                record.record_seq
            ));
        }
        if journal.deliveries.len() >= self.config.max_queue {
            return Err("QUEUE_FULL: Journal đã đầy; chỉ giải phóng receipt sau khi PROCESSED và ACK được xác nhận.".into());
        }
        let received_at = Utc::now();
        let sku = self.config.operator_sku.as_deref().ok_or("OPERATOR_SKU_REQUIRED: Chưa nhập SKU QC. Dừng thu an toàn và nhập SKU trên trang Giám sát.")?;
        let key = crate::services::inbox_service::sku_delivery_key(
            sku,
            received_at
                .timestamp_nanos_opt()
                .unwrap_or(received_at.timestamp_micros()),
            record.record_seq
        )?;
        let input = InboxInput {
            source_id: 0,
            confirmed_session_id: journal.session_id.clone(),
            ingest_key: key.clone(),
            record_seq: record.record_seq,
            total_count: Some(i64::from(control.total_count)),
            buffer_index: 0,
            raw_registers: buffer.raw_registers.clone(),
            word_order: self.config.word_order,
            received_at,
        };
        let latest = LatestRecord {
            source_key: self.config.source_key.clone(),
            session_id: journal.session_id.clone(),
            delivery_id: key,
            inbox_id: None,
            measurement_id: None,
            record_seq: record.record_seq,
            total_count: input.total_count,
            product_code: sku.to_string(),
            club_number: record.stick_no,
            received_at,
            overall_result: format!("{:?}", record.overall_result).to_uppercase(),
            tip_result: format!("{:?}", record.tip.result).to_uppercase(),
            butt_result: format!("{:?}", record.butt.result).to_uppercase(),
            save_state: "QUEUED".into(),
        };
        Ok(Some((input, latest)))
    }

    pub(super) async fn capture_published_snapshot(
        &self,
        snapshot: PublishedRecordSnapshot,
    ) -> Result<(), String> {
        let mut journal = self.journal.lock().await;
        if let Some(pause) = &journal.pause {
            return Err(format!("Bản ghi 1 đang dừng bảo vệ dữ liệu: {pause}."));
        }
        let mut next = journal.clone();
        let delivery = match self.published_delivery(&next, &snapshot) {
            Ok(delivery) => delivery,
            // Missing operator input is not corrupt PLC data. All protocol/reset
            // validation above still runs; existing deliveries retain their SKU.
            Err(error) if error.starts_with("OPERATOR_SKU_REQUIRED:") => return Err(error),
            Err(error) => {
                next.pause = Some(
                    if error.starts_with("RESET:") {
                        "RESET"
                    } else if error.starts_with("CONFLICT:") {
                        "CONFLICT"
                    } else {
                        "PUBLISHED_DIAGNOSTIC"
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
                    } else {
                        next.pause = Some("DIAGNOSTIC_FULL".into());
                    }
                }
                let persisted = self.persist(&next);
                *journal = next;
                drop(journal);
                persisted?;
                self.publish_journal().await?;
                return Err(error);
            }
        };
        next.last_total = Some(snapshot.control.total_count); // Observed counter, never ACK target.
        let latest = delivery.map(|(input, latest)| {
            next.deliveries.push(Delivery {
                input,
                inbox_id: None,
                terminal: false,
            });
            latest
        });
        // Preserve the captured raw/key in RAM even if disk persistence fails;
        // the next explicit iteration retries the same receipt before reading again.
        let persisted = self.persist(&next);
        *journal = next;
        let volatile_count = journal
            .deliveries
            .iter()
            .filter(|d| d.inbox_id.is_none())
            .count();
        drop(journal);
        let mut status = self.state.status.write().await;
        status.volatile_count = volatile_count;
        status.connection = "CONNECTED".into();
        if let Some(latest) = latest {
            status.raw_capture = None;
            if latest_replaces(status.latest.as_ref(), &latest) {
                status.latest = Some(latest);
            }
            status.state = "QUEUED".into();
            status.message = if persisted.is_ok() {
                "Đã giữ Bản ghi 1 trong Journal; chờ lưu PostgreSQL, chưa gửi ACK.".into()
            } else {
                "Chưa ghi được Journal Bản ghi 1; raw còn trong RAM, có nguy cơ mất khi đóng ứng dụng.".into()
            };
        }
        drop(status);
        persisted?;
        self.publish_journal().await
    }
}
