//! Validate and persist only the single published region.
use super::*;

impl Engine {
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
            .map(|d| d.input.record_seq)
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
            .find(|d| d.input.record_seq == record.record_seq)
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
        let key = format!(
            "published:{}:{}",
            received_at
                .timestamp_nanos_opt()
                .unwrap_or(received_at.timestamp_micros()),
            record.record_seq
        );
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
            product_code: record.product_code,
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
