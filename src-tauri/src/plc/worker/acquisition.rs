//! One raw receipt path; decoding is optional and cannot authorize ACK.
use super::*;
use store::Progress;

impl Engine {
    pub(super) async fn open(
        mut config: SimulatorConfig,
        path: PathBuf,
        state: WorkerState,
        pool: &PgPool,
    ) -> Result<Arc<Self>, String> {
        config.validate()?;
        super::super::network::pin_worker(&mut config)?;
        if !config.allow_plc_connection
            || config.confirmed_session_id.is_none()
            || config.pc_heartbeat_enabled
            || config.allow_plc_heartbeat_write
        {
            return Err(
                "Cần nguồn/phiên cấu hình rõ ràng, quyền kết nối và tắt ghi PC Heartbeat.".into(),
            );
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        // Keep the original lock path, so an older binary cannot run beside this Worker.
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))
            .map_err(|e| e.to_string())?;
        lock.try_lock()
            .map_err(|e| format!("Một Worker khác đang giữ quyền thu/ACK: {e}"))?;
        let source = SourceRepository::upsert(pool, &config.source_key, None).await?;
        let current = store::progress(pool, source.id).await?;
        if current.is_none() {
            let (accepted, anchor, last_total) =
                legacy::baseline(&config, &path, pool, source.id).await?;
            let snapshot = config
                .plc_reader()
                .poll_published_record()
                .await
                .map_err(|e| e.to_string())?;
            let c = &snapshot.control;
            let unconfirmed = c.pc_read_count != accepted
                || c.pc_read_done != 0
                || c.total_count < last_total
                || (c.data_ready == 0 && c.total_count != accepted)
                || (last_total > accepted && (c.data_ready != 1 || c.total_count != last_total));
            let blocked = unconfirmed.then_some("INITIAL_HANDSHAKE");
            // Preserve a ready raw payload even when the remote checkpoint is
            // unknown. Never adopt remote Count as our accepted checkpoint.
            sqlx::query("INSERT INTO plc_acquisition(source_id,session_id,binding,accepted,last_total,anchor_inbox_id,blocked_reason) VALUES($1,$2,$3,$4,$5,$6,$7)")
                .bind(source.id).bind(&config.confirmed_session_id).bind(legacy::binding(&config))
                .bind(accepted).bind(last_total).bind(anchor).bind(blocked).execute(pool).await.map_err(|e| e.to_string())?;
        }
        let p = store::progress(pool, source.id)
            .await?
            .ok_or("Thiếu trạng thái thu dữ liệu.")?;
        if p.binding != legacy::binding(&config)
            || Some(&p.session_id) != config.confirmed_session_id.as_ref()
        {
            return Err(
                "Trạng thái DB khác nguồn/phiên/map đang cấu hình; không tự sửa checkpoint.".into(),
            );
        }
        if let Some(id) = p.anchor_inbox_id {
            let row = InboxRepository::find(pool, id).await?;
            if row.source_id != source.id
                || row.total_count != Some(i64::from(p.accepted))
                || row.word_order != config.word_order.as_str()
                || (row.publication_session.is_some()
                    && (row.publication_session.as_deref() != Some(p.session_id.as_str())
                        || row.publication_count != Some(p.accepted)))
                || (row.publication_session.is_none()
                    && row.session_id.as_deref() != Some(p.session_id.as_str())
                    && row.ingest_key
                        != format!(
                            "raw-published-v1:ack:{}:{}:{}",
                            p.session_id.len(),
                            p.session_id,
                            p.accepted
                        ))
            {
                return Err("Thiếu receipt đã xác nhận; giữ checkpoint để đối chiếu.".into());
            }
        } else if p.accepted != 0 {
            return Err("Checkpoint thiếu receipt anchor.".into());
        }
        state.status.write().await.session_id = Some(p.session_id.clone());
        let engine = Arc::new(Self {
            config,
            source_id: source.id,
            state,
            _lock: lock,
            projection_due: Mutex::new(std::time::Instant::now()),
            projection_cursors: Mutex::new((0, 0)),
            published_gate: Mutex::new(()),
        });
        engine.refresh_display(pool).await?;
        Ok(engine)
    }

    fn same_receipt(row: &InboxRecord, snapshot: &PublishedRecordSnapshot) -> bool {
        snapshot.control.data_ready == 1
            && row.publication_count == Some(snapshot.control.total_count)
            && snapshot.buffer.as_ref().is_some_and(|b| {
                serde_json::to_value(&b.raw_registers).is_ok_and(|raw| raw == row.raw_registers)
            })
    }

    fn receipt_ack_status(&self, row: &InboxRecord, p: &Progress) -> &'static str {
        if p.pending_inbox_id == Some(row.id) {
            if p.blocked_reason.is_some()
                || row.total_count == Some(0)
                || (p.ack_attempted && p.done_started != Some(false))
            {
                return "RECONCILIATION_REQUIRED";
            }
            return if self.config.read_only || !self.config.ack_enabled {
                "DISABLED"
            } else {
                "PENDING"
            };
        }
        let count = row.total_count.and_then(|n| i32::try_from(n).ok());
        let in_prefix = count.is_some_and(|n| n > 0 && n <= p.accepted);
        let same_epoch = row.publication_session.as_deref() == Some(p.session_id.as_str())
            || (row.publication_session.is_none()
                && row.session_id.as_deref() == Some(p.session_id.as_str()))
            || count.is_some_and(|n| {
                row.ingest_key
                    == format!(
                        "raw-published-v1:ack:{}:{}:{n}",
                        p.session_id.len(),
                        p.session_id
                    )
            });
        if in_prefix && (same_epoch || p.anchor_inbox_id == Some(row.id)) {
            "ACKNOWLEDGED"
        } else {
            "RECONCILIATION_REQUIRED"
        }
    }

    pub(super) async fn refresh_display(&self, pool: &PgPool) -> Result<(), String> {
        let p = store::progress(pool, self.source_id)
            .await?
            .ok_or("Thiếu trạng thái DB.")?;
        if p.binding != legacy::binding(&self.config)
            || Some(&p.session_id) != self.config.confirmed_session_id.as_ref()
        {
            return Err("Binding DB khác cấu hình; không suy trạng thái ACK để hiển thị.".into());
        }
        let latest: Option<InboxRecord> = sqlx::query_as(
            "SELECT * FROM plc_inbox WHERE source_id=$1 ORDER BY received_at DESC,id DESC LIMIT 1",
        )
        .bind(self.source_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;
        let last_id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM measurements WHERE source_id=$1 ORDER BY id DESC LIMIT 1",
        )
        .bind(self.source_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;
        if let Some(row) = latest {
            self.show_receipt(&row, &p).await?;
        }
        if let Some(id) = p.pending_inbox_id {
            let row = self.receipt(pool, id, &p).await?;
            self.show_receipt(&row, &p).await?;
        }
        let mut status = self.state.status.write().await;
        status.last_saved_id = last_id;
        status.pause_reason = p.blocked_reason;
        if status.pause_reason.is_some() {
            status.state = "PAUSED".into();
            status.ack_status = "RECONCILIATION_REQUIRED";
        }
        Ok(())
    }

    pub(super) async fn show_receipt(&self, row: &InboxRecord, p: &Progress) -> Result<(), String> {
        let words: Vec<u16> =
            serde_json::from_value(row.raw_registers.clone()).map_err(|e| e.to_string())?;
        let order = if row.word_order == "LOW" {
            as200::WordOrder::LowWordFirst
        } else {
            as200::WordOrder::HighWordFirst
        };
        let record = decode::record(&words, 0, order).ok();
        let mut status = self.state.status.write().await;
        if let Some(id) = row.measurement_id {
            status.last_saved_id = Some(id);
        }
        if status
            .latest
            .as_ref()
            .is_some_and(|latest| latest.received_at > row.received_at)
            || status
                .raw_capture
                .as_ref()
                .is_some_and(|raw| raw.received_at > row.received_at)
        {
            return Ok(());
        }
        status.raw_capture = if row.measurement_id.is_none() {
            Some(RawCaptureStatus {
                inbox_id: row.id,
                received_at: row.received_at,
                total_count: row
                    .total_count
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or("Raw thiếu TotalCount hợp lệ.")?,
                ack_status: self.receipt_ack_status(row, p),
            })
        } else {
            None
        };
        status.ack_status = self.receipt_ack_status(row, p);
        if let Some(record) = record {
            status.latest = Some(LatestRecord {
                source_key: self.config.source_key.clone(),
                session_id: row.publication_session.clone().or(row.session_id.clone()),
                delivery_id: row.ingest_key.clone(),
                inbox_id: Some(row.id),
                measurement_id: row.measurement_id,
                record_seq: row.record_seq,
                total_count: row.total_count,
                product_code: row
                    .assigned_sku
                    .clone()
                    .or(crate::services::inbox_service::assigned_sku(
                        &row.ingest_key,
                        row.record_seq,
                    )?)
                    .unwrap_or(record.product_code),
                club_number: record.stick_no,
                received_at: row.received_at,
                overall_result: format!("{:?}", record.overall_result).to_uppercase(),
                tip_result: format!("{:?}", record.tip.result).to_uppercase(),
                butt_result: format!("{:?}", record.butt.result).to_uppercase(),
                save_state: if row.measurement_id.is_some() {
                    "PROCESSED"
                } else if row.processing_status == "FAILED" {
                    "FAILED"
                } else {
                    "RAW_UNDECODED"
                }
                .into(),
            });
        }
        if status.pause_reason.is_none() {
            status.state = if row.measurement_id.is_some() {
                "PROCESSED"
            } else {
                "RAW_UNDECODED"
            }
            .into();
            status.message = if row.measurement_id.is_some() {
                "Raw đã commit và Measurement đã lưu; xem trạng thái ACK để biết PLC đã xác nhận hay chưa."
            } else {
                "Raw chính thức đã commit; chưa tạo Measurement. Xem trạng thái ACK để biết lý do đang chờ."
            }.into();
        }
        Ok(())
    }

    pub(super) async fn process_receipt(
        &self,
        pool: &PgPool,
        row: &InboxRecord,
    ) -> Result<(), String> {
        // Invalid payloads remain raw. No retry storm and no synthetic measurement.
        if row.measurement_id.is_some() {
            return Ok(());
        }
        if let Err(error) = crate::services::inbox_service::draft_from_inbox(row) {
            sqlx::query("UPDATE plc_inbox SET decode_error=$2 WHERE id=$1 AND decode_error IS NULL AND measurement_id IS NULL")
                .bind(row.id).bind(error).execute(pool).await.map_err(|e| e.to_string())?;
            return Ok(());
        }
        sqlx::query("UPDATE plc_inbox SET processing_status='PENDING',error_message=NULL WHERE id=$1 AND processing_status='SESSION_UNCONFIRMED' AND publication_session IS NOT NULL")
            .bind(row.id).execute(pool).await.map_err(|e| e.to_string())?;
        InboxService::process(pool, row.id).await.map(|_| ())
    }

    pub(super) async fn project_pending(&self, pool: &PgPool) -> Result<(), String> {
        let mut due = self.projection_due.lock().await;
        if std::time::Instant::now() < *due || self.state.stopping.load(Ordering::SeqCst) {
            return Ok(());
        }
        *due = std::time::Instant::now() + Duration::from_millis(self.config.retry_interval_ms);
        let _gate = self.published_gate.lock().await;
        let mut cursors = self.projection_cursors.lock().await;
        let mut groups = Vec::new();
        for (failed, cursor) in [(false, cursors.0), (true, cursors.1)] {
            let rows: Vec<InboxRecord> = sqlx::query_as("SELECT * FROM plc_inbox WHERE source_id=$1 AND measurement_id IS NULL AND decode_error IS NULL AND (session_id IS NOT NULL OR publication_session IS NOT NULL) AND (processing_status='FAILED')=$2 ORDER BY (id>$3) DESC,id LIMIT 5")
                .bind(self.source_id).bind(failed).bind(cursor).fetch_all(pool).await.map_err(|e| e.to_string())?;
            groups.push(rows);
        }
        let mut errors = Vec::new();
        for index in 0..5 {
            for (group, rows) in groups.iter().enumerate() {
                if self.state.stopping.load(Ordering::SeqCst) {
                    break;
                }
                if let Some(row) = rows.get(index) {
                    if group == 0 {
                        cursors.0 = row.id;
                    } else {
                        cursors.1 = row.id;
                    }
                    if let Err(error) = self.process_receipt(pool, row).await {
                        errors.push(format!("MEASUREMENT: Inbox #{}: {error}", row.id));
                    }
                }
            }
        }
        drop(cursors);
        let (failed, waiting): (i64, i64) = sqlx::query_as("SELECT count(*) FILTER (WHERE processing_status='FAILED'),count(*) FILTER (WHERE processing_status<>'FAILED') FROM plc_inbox WHERE source_id=$1 AND measurement_id IS NULL AND decode_error IS NULL AND (session_id IS NOT NULL OR publication_session IS NOT NULL)")
            .bind(self.source_id).fetch_one(pool).await.map_err(|e| e.to_string())?;
        let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM plc_inbox WHERE source_id=$1 AND processing_status='FAILED' AND measurement_id IS NULL AND decode_error IS NULL ORDER BY id LIMIT 100")
            .bind(self.source_id).fetch_all(pool).await.map_err(|e| e.to_string())?;
        {
            let mut status = self.state.status.write().await;
            status.blocked_inbox_count = failed as usize;
            status.blocked_inbox_ids = ids;
            status.retry_waiting_count = waiting as usize;
            status.warnings.retain(|w| !w.starts_with("MEASUREMENT:"));
            status.warnings.extend(errors);
        }
        self.refresh_display(pool).await?;
        Ok(())
    }

    async fn observe_ack(
        &self,
        pool: &PgPool,
        p: &Progress,
        row: &InboxRecord,
        snapshot: &PublishedRecordSnapshot,
    ) -> Result<bool, String> {
        let c = &snapshot.control;
        let target = row
            .publication_count
            .ok_or("Receipt thiếu counter công bố.")?;
        if !p.ack_attempted || p.done_started == Some(false) {
            return Ok(false);
        }
        if c.pc_read_count == target
            && c.total_count == target
            && c.pc_read_done == 1
            && Self::same_receipt(row, snapshot)
        {
            sqlx::query("UPDATE plc_acquisition SET done_observed=true WHERE source_id=$1 AND pending_inbox_id=$2 AND ack_attempted")
                .bind(p.source_id).bind(row.id).execute(pool).await.map_err(|e| e.to_string())?;
        }
        let complete = p.done_observed
            && c.pc_read_done == 0
            && c.pc_read_count == target
            && ((c.total_count == target && c.data_ready == 0)
                || (target.checked_add(1) == Some(c.total_count)
                    && c.data_ready == 1
                    && snapshot.buffer.is_some()));
        if complete {
            let changed = sqlx::query("UPDATE plc_acquisition SET accepted=$3,last_total=GREATEST(last_total,$3),anchor_inbox_id=$2,pending_inbox_id=NULL,ack_attempted=false,done_observed=false,done_started=false WHERE source_id=$1 AND pending_inbox_id=$2 AND accepted=$4 AND session_id=$5 AND binding=$6 AND blocked_reason IS NULL AND ack_attempted AND done_observed AND done_started IS DISTINCT FROM false")
                .bind(p.source_id).bind(row.id).bind(target).bind(p.accepted).bind(&p.session_id).bind(&p.binding).execute(pool).await.map_err(|e| e.to_string())?;
            if changed.rows_affected() != 1 {
                return Err("ACK receipt thay đổi; cần đối chiếu.".into());
            }
            let mut confirmed = p.clone();
            confirmed.accepted = target;
            confirmed.pending_inbox_id = None;
            self.show_receipt(row, &confirmed).await?;
            let mut status = self.state.status.write().await;
            status.ack_status = "ACKNOWLEDGED";
            status.pause_reason = None;
            status.last_error = None;
            status.state = if row.measurement_id.is_some() {
                "PROCESSED"
            } else {
                "RAW_UNDECODED"
            }
            .into();
            status.message = "Đã lưu raw và xác nhận PLC xử lý ACK.".into();
            return Ok(true);
        }
        self.state.status.write().await.ack_status = "RECONCILIATION_REQUIRED";
        Err("Raw đã commit; ACK đã bắt đầu nhưng chưa có bằng chứng Done cao → thấp. Chỉ quan sát PLC/DB, không tự gửi lại Count hoặc Done.".into())
    }

    async fn checked_write_snapshot(
        &self,
        pool: &PgPool,
        row: &InboxRecord,
        expected_count: i32,
    ) -> Result<(), String> {
        if self.state.stopping.load(Ordering::SeqCst) {
            return Err("Đang dừng; chưa ghi PLC.".into());
        }
        let snapshot = self.read_snapshot().await?;
        if !Self::same_receipt(row, &snapshot)
            || snapshot.control.pc_read_count != expected_count
            || snapshot.control.pc_read_done != 0
        {
            self.block(pool, "SNAPSHOT_CHANGED").await?;
            return Err(
                "Publication/Count/Done thay đổi trước write; giữ receipt để đối chiếu.".into(),
            );
        }
        if self.state.stopping.load(Ordering::SeqCst) || !self.state.fresh_health().await {
            return Err("Đã giữ raw; đang dừng hoặc heartbeat chưa RUNNING, chưa ghi PLC.".into());
        }
        Ok(())
    }

    // Only called by the still-running owner BEFORE invoking the transport write.
    // Never called after a timeout/response error, or as a restart recovery shortcut.
    async fn release_unsent_marker(
        &self,
        pool: &PgPool,
        p: &Progress,
        row: &InboxRecord,
        done: bool,
    ) -> Result<(), String> {
        let sql = if done {
            "UPDATE plc_acquisition SET done_started=false WHERE source_id=$1 AND pending_inbox_id=$2 AND accepted=$3 AND session_id=$4 AND binding=$5 AND ack_attempted AND done_started=true AND NOT done_observed"
        } else {
            "UPDATE plc_acquisition SET ack_attempted=false WHERE source_id=$1 AND pending_inbox_id=$2 AND accepted=$3 AND session_id=$4 AND binding=$5 AND ack_attempted AND done_started=false AND NOT done_observed"
        };
        let changed = sqlx::query(sql)
            .bind(p.source_id)
            .bind(row.id)
            .bind(p.accepted)
            .bind(&p.session_id)
            .bind(&p.binding)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;
        if changed.rows_affected() != 1 {
            return Err(
                "Không xác nhận được marker chưa gửi; chỉ đối chiếu, không tự reset.".into(),
            );
        }
        Ok(())
    }

    fn ack_writer(&self) -> Result<as200::ack::AckWriter, String> {
        as200::ack::AckWriter::new(
            self.config.reader(),
            &self.config.mode,
            self.config.ack_enabled,
            self.config.ack_protocol.as_deref(),
            self.config.allow_plc_connection,
        )
        .map(|writer| writer.with_local_ip(self.config.plc_local_ip))
    }

    async fn first_done(
        &self,
        pool: &PgPool,
        p: &Progress,
        row: &InboxRecord,
    ) -> Result<(), String> {
        if !p.ack_attempted || p.done_started != Some(false) {
            return Err("Không có bằng chứng Done chưa bắt đầu; không gửi lại Done.".into());
        }
        if self.state.stopping.load(Ordering::SeqCst) {
            return Err("Đang dừng trước Done.".into());
        }
        let target = row.publication_count.ok_or("Thiếu publication counter.")?;
        let writer = self.ack_writer()?;
        let stream = writer.prepare_done().await?;
        self.checked_write_snapshot(pool, row, target).await?;
        let changed = sqlx::query("UPDATE plc_acquisition SET done_started=true WHERE source_id=$1 AND pending_inbox_id=$2 AND accepted=$3 AND session_id=$4 AND binding=$5 AND blocked_reason IS NULL AND ack_attempted AND done_started=false AND NOT done_observed")
            .bind(p.source_id).bind(row.id).bind(p.accepted).bind(&p.session_id).bind(&p.binding)
            .execute(pool).await.map_err(|e| e.to_string())?;
        if changed.rows_affected() != 1 {
            return Err("Done đã bắt đầu hoặc trạng thái DB đổi; không gửi lặp.".into());
        }
        if let Err(error) = self.checked_write_snapshot(pool, row, target).await {
            self.release_unsent_marker(pool, p, row, true).await?;
            return Err(error);
        }
        // Any interruption from this call onward leaves done_started=true.
        writer.done_prepared(stream).await?;
        let snapshot = self.read_snapshot().await?;
        let mut sent = p.clone();
        sent.done_started = Some(true);
        self.observe_ack(pool, &sent, row, &snapshot)
            .await
            .map(|_| ())
    }

    pub(super) async fn poll_and_ack_published(&self, pool: &PgPool) -> Result<(), String> {
        let _gate = self.published_gate.lock().await;
        let mut p = store::progress(pool, self.source_id)
            .await?
            .ok_or("Thiếu trạng thái DB; dừng ACK.")?;
        if p.binding != legacy::binding(&self.config)
            || Some(&p.session_id) != self.config.confirmed_session_id.as_ref()
        {
            return Err("Binding DB thay đổi; không ACK.".into());
        }
        let snapshot = self.read_snapshot().await?;
        let c = &snapshot.control;
        if p.pending_inbox_id.is_none() && c.data_ready == 1 {
            self.receive(pool, &p, &snapshot, None).await?;
            p = store::progress(pool, self.source_id)
                .await?
                .ok_or("Thiếu trạng thái sau commit.")?;
        }
        let mut pending = if let Some(id) = p.pending_inbox_id {
            Some(self.receipt(pool, id, &p).await?)
        } else {
            None
        };
        if let Some(row) = &pending {
            self.show_receipt(row, &p).await?;
        }
        if pending
            .as_ref()
            .is_some_and(|row| row.publication_count == Some(0))
            && p.accepted == 0
            && !p.ack_attempted
            && p.blocked_reason.is_none()
            && c.total_count == 1
            && c.pc_read_count == 0
            && c.pc_read_done == 0
            && c.data_ready == 1
        {
            // Keep row 0 unchanged as an unconfirmed observation. Only the new
            // positive PLC publication becomes pending, in the same transaction.
            self.receive(pool, &p, &snapshot, p.pending_inbox_id)
                .await?;
            p = store::progress(pool, self.source_id)
                .await?
                .ok_or("Thiếu trạng thái sau chuyển publication.")?;
            pending = Some(
                self.receipt(pool, p.pending_inbox_id.ok_or("Thiếu receipt mới.")?, &p)
                    .await?,
            );
            if let Some(row) = &pending {
                self.show_receipt(row, &p).await?;
            }
        }
        if let Some(reason) = &p.blocked_reason {
            let mut status = self.state.status.write().await;
            status.pause_reason = Some(reason.clone());
            status.ack_status = "RECONCILIATION_REQUIRED";
            return Err(format!(
                "DB đang giữ trạng thái cần đối chiếu: {reason}; không tự xóa block/đổi epoch."
            ));
        }
        if c.total_count < p.last_total || c.pc_read_count < p.accepted {
            self.block(pool, "RESET").await?;
            return Err("Counter PLC giảm; không tự tạo epoch hoặc sửa checkpoint.".into());
        }
        if c.total_count > p.last_total {
            sqlx::query(
                "UPDATE plc_acquisition SET last_total=$2 WHERE source_id=$1 AND last_total<$2",
            )
            .bind(p.source_id)
            .bind(c.total_count)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;
        }
        let Some(row) = pending else {
            if c.data_ready != 0
                || c.total_count != p.accepted
                || c.pc_read_count != p.accepted
                || c.pc_read_done != 0
            {
                return Err("PLC idle nhưng counter/Done khác DB; chưa thể xác nhận.".into());
            }
            let mut status = self.state.status.write().await;
            status.state = if status.raw_capture.is_some() {
                "RAW_UNDECODED"
            } else if status.latest.is_some() {
                "PROCESSED"
            } else {
                "WAITING"
            }
            .into();
            status.pause_reason = None;
            status.last_error = None;
            status.message = "Đã kết nối máy đo, đang chờ publication mới; trạng thái ACK của bản ghi cũ được giữ nguyên.".into();
            return Ok(());
        };
        let target = row.publication_count.ok_or("Thiếu định danh công bố.")?;
        if target <= 0 || p.accepted.checked_add(1) != Some(target) {
            self.state.status.write().await.ack_status = "RECONCILIATION_REQUIRED";
            return Err(if target == 0 && c.total_count == 0 && !Self::same_receipt(&row, &snapshot) {
                "Payload counter 0 đã đổi; giữ raw đầu trong DB. Chưa có identity mới để lưu/gửi ACK."
            } else { "Đã giữ raw; chưa chứng minh publication kế tiếp. Không đổi counter hoặc bỏ dữ liệu để ACK." }.into());
        }
        if p.ack_attempted && p.done_started != Some(false) {
            return self
                .observe_ack(pool, &p, &row, &snapshot)
                .await
                .map(|_| ());
        }
        if self.config.read_only || !self.config.ack_enabled {
            self.state.status.write().await.ack_status = "DISABLED";
            return Ok(());
        }
        if !Self::same_receipt(&row, &snapshot) {
            self.block(pool, "PUBLICATION_CONFLICT").await?;
            return Err("Raw/target khác receipt đã commit; không ACK.".into());
        }
        if p.ack_attempted {
            if c.pc_read_count != target || c.pc_read_done != 0 {
                return Err("Count chưa được đối chiếu đúng target hoặc Done khác 0; tiếp tục quan sát, không phát lại Count.".into());
            }
            return self.first_done(pool, &p, &row).await;
        }
        if c.pc_read_count != p.accepted || c.pc_read_done != 0 {
            return Err("Count/Done khác điểm bắt đầu đã xác nhận; chưa gửi ACK.".into());
        }
        if self.state.stopping.load(Ordering::SeqCst) {
            return Err("Đang dừng trước Count.".into());
        }
        let writer = self.ack_writer()?;
        let stream = writer.prepare_count().await?;
        self.checked_write_snapshot(pool, &row, p.accepted).await?;
        let changed = sqlx::query("UPDATE plc_acquisition SET ack_attempted=true WHERE source_id=$1 AND pending_inbox_id=$2 AND accepted=$3 AND session_id=$4 AND binding=$5 AND blocked_reason IS NULL AND NOT ack_attempted AND done_started=false AND NOT done_observed")
            .bind(p.source_id).bind(row.id).bind(p.accepted).bind(&p.session_id).bind(&p.binding)
            .execute(pool).await.map_err(|e| e.to_string())?;
        if changed.rows_affected() != 1 {
            return Err("ACK đã bắt đầu hoặc trạng thái đổi; không gửi lặp.".into());
        }
        p.ack_attempted = true;
        if let Err(error) = self.checked_write_snapshot(pool, &row, p.accepted).await {
            self.release_unsent_marker(pool, &p, &row, false).await?;
            return Err(error);
        }
        writer.count_prepared(target, stream).await?;
        self.first_done(pool, &p, &row).await
    }
}

impl WorkerState {
    pub(in crate::plc) async fn verify_sku_change(
        &self,
        directory: &std::path::Path,
        config: &SimulatorConfig,
        pool: &PgPool,
    ) -> Result<Option<File>, String> {
        self.ensure_register_writes_stopped().await?;
        let path = config.journal_file(&directory.join("as200_config.toml"));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))
            .map_err(|e| e.to_string())?;
        lock.try_lock().map_err(|e| e.to_string())?;
        let source: Option<i64> =
            sqlx::query_scalar("SELECT id FROM plc_sources WHERE source_key=$1")
                .bind(&config.source_key)
                .fetch_optional(pool)
                .await
                .map_err(|e| e.to_string())?;
        let p = match source {
            Some(id) => store::progress(pool, id).await?,
            None => None,
        };
        let accepted = if let Some(p) = p {
            if p.pending_inbox_id.is_some()
                || p.blocked_reason.is_some()
                || p.last_total != p.accepted
                || p.binding != legacy::binding(config)
            {
                return Err("Còn raw/ACK chưa xác nhận hoặc binding khác; chưa đổi SKU.".into());
            }
            p.accepted
        } else if let Some(source) = source {
            let (accepted, _, last_total) = legacy::baseline(config, &path, pool, source).await?;
            if last_total != accepted {
                return Err(
                    "Journal đã thấy publication kế tiếp; giữ SKU cho tới khi tiếp nhận/ACK xong."
                        .into(),
                );
            }
            accepted
        } else if path.exists() {
            return Err("Còn Journal nhưng thiếu source DB; chưa đổi SKU.".into());
        } else {
            0
        };
        let mut network_config = config.clone();
        super::super::network::pin_worker(&mut network_config)?;
        for _ in 0..2 {
            let snapshot = network_config
                .plc_reader()
                .poll_published_record()
                .await
                .map_err(|e| e.to_string())?;
            let c = snapshot.control;
            if c.data_ready != 0
                || c.pc_read_done != 0
                || c.pc_read_count != accepted
                || c.total_count != accepted
            {
                return Err("PLC còn bản ghi hoặc handshake khác DB; giữ SKU cũ.".into());
            }
        }
        Ok(Some(lock))
    }
}
