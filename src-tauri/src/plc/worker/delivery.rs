//! Durable receipts and PostgreSQL processing/retry, independent of PLC writes.
use super::*;

impl Engine {
    pub(super) fn matches(
        row: &InboxRecord,
        input: &InboxInput,
        source_id: i64,
    ) -> Result<(), String> {
        if row.source_id != source_id
            || row.record_seq != input.record_seq
            || row.raw_registers
                != serde_json::to_value(&input.raw_registers).map_err(|e| e.to_string())?
            || row.word_order != input.word_order.as_str()
            || (input.confirmed_session_id.is_some()
                && row.session_id != input.confirmed_session_id)
        {
            return Err("CONFLICT: Inbox đã có nhưng source/session/sequence/raw/WordOrder không khớp; không ghi đè hoặc đổi delivery ID.".into());
        }
        Ok(())
    }

    pub(super) async fn row_status(&self, row: &InboxRecord) -> Result<(), String> {
        // Retry from an archived session must not replace the current session's display.
        if row.session_id != self.journal.lock().await.session_id {
            return Ok(());
        }
        let alias = self
            .journal
            .lock()
            .await
            .deliveries
            .iter()
            .find(|d| d.inbox_id == Some(row.id))
            .map(|d| d.input.ingest_key.clone());
        let mut status = self.state.status.write().await;
        let record = decode::record(
            &serde_json::from_value::<Vec<u16>>(row.raw_registers.clone())
                .map_err(|e| e.to_string())?,
            row.buffer_index as usize,
            match row.word_order.as_str() {
                "LOW" => super::as200::WordOrder::LowWordFirst,
                "HIGH" => super::as200::WordOrder::HighWordFirst,
                _ => return Err("WordOrder inbox không hợp lệ".into()),
            },
        )
        .map_err(|e| e.to_string())?;
        record.validate_metadata(row.total_count)?;
        if record.record_seq != row.record_seq {
            return Err("AS200_METADATA: inbox/raw sequence mismatch".into());
        }
        if !record.complete() {
            return Err(
                "Inbox chưa phải bản ghi hoàn thành; không tự xử lý thành measurement.".into(),
            );
        }
        let current = status.latest.as_ref().is_none_or(|current| {
            if row.session_id.is_some()
                && current.source_key == self.config.source_key
                && current.session_id == row.session_id
            {
                row.record_seq >= current.record_seq
            } else if current.session_id != row.session_id {
                true // Incoming session has already been checked against the accepted journal.
            } else {
                current.received_at <= row.received_at
                    || current.inbox_id == Some(row.id)
                    || alias.as_ref() == Some(&current.delivery_id)
            }
        });
        if !current {
            return Ok(());
        }
        if let Some(id) = row.measurement_id {
            status.last_saved_id = Some(id);
        }
        if current {
            status.latest = Some(LatestRecord {
                source_key: self.config.source_key.clone(),
                session_id: row.session_id.clone(),
                delivery_id: row.ingest_key.clone(),
                inbox_id: Some(row.id),
                measurement_id: row.measurement_id,
                record_seq: row.record_seq,
                total_count: row.total_count,
                product_code: record.product_code,
                club_number: record.stick_no,
                received_at: row.received_at,
                overall_result: format!("{:?}", record.overall_result).to_uppercase(),
                tip_result: format!("{:?}", record.tip.result).to_uppercase(),
                butt_result: format!("{:?}", record.butt.result).to_uppercase(),
                save_state: row.processing_status.clone(),
            });
        }
        status.state = row.processing_status.clone();
        status.message = match row.processing_status.as_str() {
            "PROCESSED" => format!(
                "Đã commit PostgreSQL. Mã bản ghi: {}",
                row.measurement_id.unwrap()
            ),
            "SESSION_UNCONFIRMED" => {
                "Đã giữ raw trong inbox; chưa xác nhận phiên, không tạo measurement.".into()
            }
            "FAILED" => row
                .error_message
                .clone()
                .unwrap_or_else(|| "Xử lý inbox thất bại; sẽ retry cùng inbox ID.".into()),
            _ => "Đã lưu inbox, đang chờ xử lý PostgreSQL.".into(),
        };
        Ok(())
    }

    pub(super) async fn persist_receipts(&self, pool: &PgPool) -> Result<i64, String> {
        let source = SourceRepository::upsert(pool, &self.config.source_key, None).await?;
        let active_session = self.journal.lock().await.session_id.clone();
        if let Some(session) = &active_session {
            if let Some(previous) = InboxRepository::session_total(pool, source.id, session).await?
            {
                let mut journal = self.journal.lock().await;
                if journal
                    .last_total
                    .is_some_and(|current| i64::from(current) < previous)
                {
                    journal.pause = Some("RESET".into());
                    Self::warn(&mut journal,"RESET/CONFLICT: TotalCount thấp hơn lịch sử inbox trong cùng session. Receipt mới được cách ly SESSION_UNCONFIRMED; cần đối chiếu phiên.".into());
                    for delivery in &mut journal.deliveries {
                        if delivery.inbox_id.is_none() {
                            delivery.input.confirmed_session_id = None;
                        }
                    }
                }
            }
        }
        let deliveries = self.journal.lock().await.deliveries.clone();
        for delivery in &deliveries {
            let mut input = delivery.input.clone();
            input.source_id = source.id;
            let existing = if let Some(id) = delivery.inbox_id {
                let found = InboxRepository::find_optional(pool, id).await?;
                if found.is_none() {
                    let message = format!(
                        "LOCAL_HISTORY_MISSING: journal inbox ID {id} không còn trong database; không tự khôi phục hoặc ACK. Cần kỹ thuật đối chiếu database, cấu hình và phiên thiết bị."
                    );
                    let mut journal = self.journal.lock().await;
                    journal.pause = Some("LOCAL_HISTORY_MISSING".into());
                    Self::warn(&mut journal, message.clone());
                    drop(journal);
                    self.publish_journal().await?;
                    return Err(message);
                }
                found
            } else {
                InboxRepository::existing_delivery(pool, source.id, &input).await?
            };
            let row = if let Some(row) = existing {
                Self::matches(&row, &input, source.id)?;
                row
            } else {
                InboxService::receive(pool, &input).await?
            };
            {
                let mut journal = self.journal.lock().await;
                if let Some(item) = journal
                    .deliveries
                    .iter_mut()
                    .find(|d| d.input.ingest_key == input.ingest_key)
                {
                    item.inbox_id = Some(row.id);
                    item.terminal = row.processing_status == "PROCESSED"
                        || row.processing_status == "SESSION_UNCONFIRMED";
                }
            }
            if let Err(error) = self.row_status(&row).await {
                // Raw đã vào inbox, lỗi payload cũ không được chặn sink cho receipt khác.
                if row.session_id.is_some() && row.measurement_id.is_none() {
                    {
                        let mut journal = self.journal.lock().await;
                        if !journal
                            .retries
                            .iter()
                            .any(|e| e.inbox_id == row.id && e.source_id == row.source_id)
                        {
                            retry::reserve(
                                &mut journal.retries,
                                &row,
                                Utc::now(),
                                self.config.retry_interval_ms,
                                self.config.retry_max_delay_ms,
                            );
                            self.persist(&journal)?;
                        }
                    }
                    self.retry_failure(&row, &error, true).await?;
                    if row.processing_status != "FAILED" {
                        InboxRepository::failed(pool, row.id, &error).await?;
                    }
                } else {
                    let mut journal = self.journal.lock().await;
                    Self::warn(
                        &mut journal,
                        format!(
                            "INBOX_DIAGNOSTIC: source={}, inbox={}, session={:?}, status={}, error={error}. Giữ nguyên dữ liệu.",
                            row.source_id, row.id, row.session_id, row.processing_status
                        ),
                    );
                }
            }
        }
        // Sync receipt IDs before processing. ACK separately proves PROCESSED by DB read-back.
        self.publish_journal().await?;
        Ok(source.id)
    }

    pub(super) async fn retry_failure(
        &self,
        row: &InboxRecord,
        error: &str,
        permanent: bool,
    ) -> Result<(), String> {
        let mut journal = self.journal.lock().await;
        if let Some(entry) = journal
            .retries
            .iter_mut()
            .find(|e| e.inbox_id == row.id && e.source_id == row.source_id)
        {
            entry.blocked = permanent || entry.attempts >= self.config.retry_max_attempts;
            entry.error = error.to_string();
            let blocked = entry.blocked;
            if blocked {
                for d in &mut journal.deliveries {
                    if d.inbox_id == Some(row.id) {
                        d.terminal = true;
                    } // Raw vẫn ở inbox, không phải PROCESSED.
                }
                Self::warn(
                    &mut journal,
                    format!(
                        "INBOX_RETRY_BLOCKED: inbox={}, source={}, session={:?}. Dữ liệu được giữ; cần kỹ thuật kiểm tra rồi yêu cầu thử lại.",
                        row.id, row.source_id, row.session_id
                    ),
                );
            }
        }
        self.persist(&journal)
    }

    pub(super) async fn process_pending(
        &self,
        pool: &PgPool,
        source_id: i64,
    ) -> Result<(), String> {
        self.process_pending_using(&retry::PostgresBackend(pool), source_id, Utc::now())
            .await
    }

    pub(super) async fn process_pending_using<B: retry::InboxBackend>(
        &self,
        backend: &B,
        source_id: i64,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        let ids: Vec<i64> = self
            .journal
            .lock()
            .await
            .retries
            .iter()
            .filter(|e| e.source_id == source_id)
            .map(|e| e.inbox_id)
            .collect();
        if !ids.is_empty() {
            // Commit có thể đã thành công nhưng client mất response/readback.
            // Đối chiếu DB trước khi báo còn FAILED hoặc thử transaction lần nữa.
            for row in backend.completed(source_id, ids).await? {
                if row.source_id != source_id
                    || row.processing_status != "PROCESSED"
                    || row.measurement_id.is_none()
                    || row.processed_at.is_none()
                {
                    return Err("CONFLICT: retry readback không phải commit hợp lệ".into());
                }
                {
                    let journal = self.journal.lock().await;
                    if journal.retries.iter().any(|e| {
                        e.source_id == source_id
                            && e.inbox_id == row.id
                            && e.session_id != row.session_id
                    }) {
                        return Err("CONFLICT: phiên inbox đã commit không khớp lịch retry".into());
                    }
                }
                if let Err(error) = self.row_status(&row).await {
                    eprintln!("AS200 committed inbox diagnostic id={}: {error}", row.id);
                }
                let mut journal = self.journal.lock().await;
                journal
                    .retries
                    .retain(|e| !(e.source_id == source_id && e.inbox_id == row.id));
                for d in &mut journal.deliveries {
                    if d.inbox_id == Some(row.id) {
                        d.terminal = true;
                    }
                }
                self.persist(&journal)?;
            }
        }
        let excluded = retry::excluded(
            &self.journal.lock().await.retries,
            source_id,
            now,
            self.config.retry_max_attempts,
        );
        let rows = backend.candidates(source_id, excluded).await?;
        let mut errors = Vec::new();
        for row in rows {
            if row.source_id != source_id || row.session_id.is_none() {
                return Err(
                    "CONFLICT: retry backend trả dữ liệu ngoài nguồn/phiên được phép".into(),
                );
            }
            {
                let mut journal = self.journal.lock().await;
                retry::reserve(
                    &mut journal.retries,
                    &row,
                    now,
                    self.config.retry_interval_ms,
                    self.config.retry_max_delay_ms,
                );
                self.persist(&journal)?; // Reservation qua restart/timeout trước khi chạy transaction.
            }
            let validation =
                crate::services::inbox_service::draft_from_inbox(&row).and_then(|draft| {
                    if draft.overall_result == crate::enums::MeasurementResult::Unmeasured
                        || draft
                            .stations
                            .iter()
                            .any(|s| s.result == crate::enums::MeasurementResult::Unmeasured)
                    {
                        Err("Dữ liệu chưa hoàn tất; cần đối chiếu quy trình PLC".into())
                    } else {
                        Ok(())
                    }
                });
            if let Err(error) = validation {
                self.retry_failure(&row, &error, true).await?;
                backend.fail(row.id, &error).await?;
                errors.push(error);
                continue;
            }
            self.row_status(&row).await?;
            {
                let mut status = self.state.status.write().await;
                status.state = "SAVING".into();
                status.message = format!(
                    "Đang xử lý inbox {} và lưu transaction PostgreSQL...",
                    row.id
                );
            }
            match backend.process(row.id).await {
                Ok(updated) => {
                    self.row_status(&updated).await?;
                    let mut journal = self.journal.lock().await;
                    journal
                        .retries
                        .retain(|e| !(e.inbox_id == row.id && e.source_id == source_id));
                    for item in &mut journal.deliveries {
                        if item.inbox_id == Some(row.id) {
                            item.terminal = true;
                        }
                    }
                    self.persist(&journal)?;
                }
                Err(error) => {
                    // Không phân loại SQL bằng chuỗi lỗi: lỗi chưa rõ được backoff hữu hạn.
                    self.retry_failure(&row, &error, false).await?;
                    backend.fail(row.id, &error).await?;
                    let mut status = self.state.status.write().await;
                    if let Some(latest) = status
                        .latest
                        .as_mut()
                        .filter(|r| r.inbox_id == Some(row.id))
                    {
                        latest.save_state = "FAILED".into();
                    }
                    errors.push(error);
                }
            }
        }
        self.publish_journal().await?;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    pub(super) async fn queue_retry(&self, id: i64) -> Result<(), String> {
        let mut journal = self.journal.lock().await;
        let mut next = journal.clone();
        if next
            .manual_retries
            .iter()
            .filter(|r| matches!(r.state.as_str(), "PENDING" | "WAITING"))
            .count()
            >= 100
        {
            return Err("Đã có nhiều yêu cầu thử lại đang chờ".into());
        }
        next.manual_retries.retain(|r| r.inbox_id != id);
        next.manual_retries.push(ManualRetryRequest {
            inbox_id: id,
            state: "PENDING".into(),
            error: None,
        });
        self.persist(&next)?;
        *journal = next;
        Ok(())
    }

    pub(super) async fn retry_result(
        &self,
        id: i64,
        state: &str,
        error: Option<String>,
    ) -> Result<(), String> {
        let mut journal = self.journal.lock().await;
        let mut next = journal.clone();
        if let Some(r) = next.manual_retries.iter_mut().find(|r| r.inbox_id == id) {
            r.state = state.into();
            r.error = error;
        }
        self.persist(&next)?;
        *journal = next;
        Ok(())
    }

    pub(super) async fn apply_retry_requests(
        &self,
        pool: &PgPool,
        source_id: i64,
    ) -> Result<(), String> {
        self.apply_retry_requests_using(&retry::PostgresBackend(pool), source_id)
            .await
    }

    pub(super) async fn apply_retry_requests_using(
        &self,
        backend: &impl retry::RetryLookup,
        source_id: i64,
    ) -> Result<(), String> {
        let requests: Vec<i64> = self
            .journal
            .lock()
            .await
            .manual_retries
            .iter()
            .filter(|r| matches!(r.state.as_str(), "PENDING" | "WAITING"))
            .map(|r| r.inbox_id)
            .collect();
        for id in requests {
            match backend.find(id).await {
                Err(error) => self.retry_result(id, "WAITING", Some(error)).await?,
                Ok(None) => {
                    self.retry_result(id, "REJECTED", Some("Không tìm thấy inbox".into()))
                        .await?
                }
                Ok(Some(row)) if row.source_id != source_id || row.session_id.is_none() => {
                    self.retry_result(
                        id,
                        "REJECTED",
                        Some("Nguồn hoặc phiên chưa được xác nhận".into()),
                    )
                    .await?;
                }
                Ok(Some(row)) => match self.apply_retry_row(&row, source_id).await {
                    Ok(()) => self.retry_result(id, "APPLIED", None).await?,
                    Err(error) => self.retry_result(id, "WAITING", Some(error)).await?,
                },
            }
        }
        self.publish_journal().await
    }

    pub(super) async fn apply_retry_row(
        &self,
        row: &InboxRecord,
        source_id: i64,
    ) -> Result<(), String> {
        if row.source_id != source_id || row.session_id.is_none() {
            return Err("Không thử lại inbox ngoài nguồn hoặc chưa xác nhận phiên".into());
        }
        if row.processing_status == "PROCESSED" {
            self.row_status(row).await?;
        }
        let mut journal = self.journal.lock().await;
        let mut next = journal.clone();
        next.retries
            .retain(|e| !(e.inbox_id == row.id && e.source_id == source_id));
        Self::warn(
            &mut next,
            format!(
                "MANUAL_RETRY: source={}, inbox={}, session={:?}, lúc {}. Không thay raw/identity/checkpoint.",
                source_id,
                row.id,
                row.session_id,
                Utc::now()
            ),
        );
        self.persist(&next)?;
        *journal = next;
        Ok(())
    }

    pub(super) async fn flush(&self, pool: &PgPool) -> Result<(), String> {
        let source = self.persist_receipts(pool).await?;
        self.apply_retry_requests(pool, source).await?;
        self.process_pending(pool, source).await
    }
}
