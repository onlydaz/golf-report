//! Committed raw receipt and linked Measurement precede new ACK writes.
use super::*;
use as200::model::PlcControl;
use store::Progress;

impl Engine {
    pub(super) fn trace_checkpoint_context(&self, p: &Progress, row: Option<&InboxRecord>) {
        super::super::diagnostics::update(|context| {
            context.inbox = row.map(|row| row.id).or(p.pending_inbox_id);
            context.measurement = row.and_then(|row| row.measurement_id);
            context.record_seq = row.map(|row| row.record_seq);
            context.target = row.and_then(|row| row.publication_count);
            context.checkpoint = format!("loaded_accepted={} expected_target={:?} loaded_last_total={} pending_inbox_id={:?} anchor={:?} ack_attempted={} done_started={:?} done_observed={} block={:?} session_matches={} binding_matches={}",
                p.accepted, p.accepted.checked_add(1), p.last_total, p.pending_inbox_id,
                p.anchor_inbox_id, p.ack_attempted, p.done_started, p.done_observed,
                p.blocked_reason, Some(&p.session_id) == self.config.confirmed_session_id.as_ref(),
                p.binding == legacy::binding(&self.config));
        });
    }

    pub(super) async fn open(
        config: SimulatorConfig,
        path: PathBuf,
        state: WorkerState,
        pool: &PgPool,
    ) -> Result<Arc<Self>, String> {
        config.validate()?;
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
            super::super::diagnostics::update(|context| {
                context.phase = "INITIAL_HANDSHAKE_CHECK";
                context.target = Some(c.total_count);
                context.control = format!("{c:?}");
            });
            crate::plc::diagnostics::event!("PLC_FC03 stage=INITIAL_CHECKPOINT_SNAPSHOT control={c:?} buffer_words={} diagnostics={:?} baseline_accepted={accepted} baseline_last_total={last_total} transport_not_called_by_open=true",
                snapshot.buffer.as_ref().map_or(0, |buffer| buffer.raw_registers.len()), snapshot.diagnostics);
            let unconfirmed = c.pc_read_count != accepted
                || c.pc_read_done != 0
                || (c.data_ready == 1 && c.total_count < last_total)
                || (c.data_ready == 0
                    && !Self::idle_control_matches_checkpoint(c, accepted, last_total))
                || (last_total > accepted && (c.data_ready != 1 || c.total_count != last_total));
            let blocked = unconfirmed.then_some("INITIAL_HANDSHAKE");
            // Preserve a ready raw payload even when the remote checkpoint is
            // unknown. Never adopt remote Count as our accepted checkpoint.
            sqlx::query("INSERT INTO plc_acquisition(source_id,session_id,binding,accepted,last_total,anchor_inbox_id,blocked_reason) VALUES($1,$2,$3,$4,$5,$6,$7)")
                .bind(source.id).bind(&config.confirmed_session_id).bind(legacy::binding(&config))
                .bind(accepted).bind(last_total).bind(anchor).bind(blocked).execute(pool).await.map_err(|e| { crate::plc::diagnostics::sql_failure(concat!(file!(), ":", line!()), &e); e.to_string() })?;
            if let Some(reason) = blocked {
                crate::plc::diagnostics::event!("PLC_ACK_CHECKPOINT stage=NEW_BLOCK_COMMITTED reason={reason} accepted={accepted} last_total={last_total} actual_count={} actual_done={} actual_total={} ready={} count_matches={} done_matches={} fc16_not_called=true fc06_not_called=true",
                    c.pc_read_count, c.pc_read_done, c.total_count, c.data_ready,
                    c.pc_read_count == accepted, c.pc_read_done == 0);
            }
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
        state.reset_diagnostic_logs();
        if let Some(old) = current.as_ref().filter(|old| old.blocked_reason.is_some()) {
            crate::plc::diagnostics::event!("PLC_ACK_CHECKPOINT stage=RECOVERED_EXISTING_BLOCK source={} session={} endpoint={} accepted={} last_total={} pending_inbox_id={:?} ack_attempted={} done_started={:?} done_observed={} reason={:?} original_failure_snapshot=not_available current_snapshot_is_not_original_failure=true transport_not_called_by_open=true",
                config.source_key, old.session_id, config.address, old.accepted, old.last_total,
                old.pending_inbox_id, old.ack_attempted, old.done_started, old.done_observed, old.blocked_reason);
        } else if let Some(old) = current.as_ref().filter(|old| old.ack_attempted) {
            crate::plc::diagnostics::event!("PLC_ACK_CHECKPOINT stage=RECOVERED_PENDING_ACK source={} session={} endpoint={} accepted={} last_total={} pending_inbox_id={:?} ack_attempted={} done_started={:?} done_observed={} previous_transport_result=unknown_from_markers original_failure_snapshot=not_available transport_not_called_by_open=true",
                config.source_key, old.session_id, config.address, old.accepted, old.last_total,
                old.pending_inbox_id, old.ack_attempted, old.done_started, old.done_observed);
        }
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

    // Only for a source with no pending receipt/ACK. While Ready is low, TotalCount
    // belongs to PLC preparation, not to the durable publication high-water mark.
    fn idle_control_matches_checkpoint(c: &PlcControl, accepted: i32, last_total: i32) -> bool {
        c.data_ready == 0
            && last_total == accepted
            && c.pc_read_count == accepted
            && c.pc_read_done == 0
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
        let draft = crate::services::inbox_service::draft_from_inbox(row).ok();
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
        if let Some(draft) = draft {
            status.latest = Some(LatestRecord {
                source_key: self.config.source_key.clone(),
                session_id: row.publication_session.clone().or(row.session_id.clone()),
                delivery_id: row.ingest_key.clone(),
                inbox_id: Some(row.id),
                measurement_id: row.measurement_id,
                record_seq: row.record_seq,
                total_count: row.total_count,
                product_code: draft.product_code,
                club_number: draft.club_number,
                received_at: row.received_at,
                quality_error: draft.quality_error,
                overall_result: draft.overall_result.as_str().into(),
                tip_result: draft
                    .stations
                    .iter()
                    .find(|s| s.station_type == crate::models::StationType::Tip)
                    .map(|s| s.result.as_str())
                    .unwrap_or("UNMEASURED")
                    .into(),
                butt_result: draft
                    .stations
                    .iter()
                    .find(|s| s.station_type == crate::models::StationType::Butt)
                    .map(|s| s.result.as_str())
                    .unwrap_or("UNMEASURED")
                    .into(),
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
        // Receipt identity errors remain explicit; incomplete quality is a stored observation.
        if row.measurement_id.is_some() {
            return Ok(());
        }
        super::super::diagnostics::update(|context| {
            context.inbox = Some(row.id);
            context.measurement = row.measurement_id;
            context.target = row.publication_count;
            context.record_seq = Some(row.record_seq);
            context.session = row.publication_session.clone().or(row.session_id.clone()).unwrap_or_default();
            context.phase = "MEASUREMENT_COMMIT";
        });
        if let Err(error) = crate::services::inbox_service::draft_from_inbox(row) {
            crate::plc::diagnostics::changed_event!("MEASUREMENT_VALIDATION", "PLC_ACK_CHECKPOINT stage=MEASUREMENT_VALIDATION_FAILED inbox={} error={error} measurement_not_created=true ack_withheld=true", row.id);
            sqlx::query("UPDATE plc_inbox SET decode_error=$2 WHERE id=$1 AND decode_error IS NULL AND measurement_id IS NULL")
                .bind(row.id).bind(error).execute(pool).await.map_err(|e| { crate::plc::diagnostics::sql_failure(concat!(file!(), ":", line!()), &e); e.to_string() })?;
            return Ok(());
        }
        sqlx::query("UPDATE plc_inbox SET processing_status=CASE WHEN processing_status='SESSION_UNCONFIRMED' THEN 'PENDING' ELSE processing_status END,decode_error=NULL WHERE id=$1 AND measurement_id IS NULL AND publication_session IS NOT NULL")
            .bind(row.id).execute(pool).await.map_err(|e| { crate::plc::diagnostics::sql_failure(concat!(file!(), ":", line!()), &e); e.to_string() })?;
        let result = InboxService::process(pool, row.id).await;
        match &result {
            Ok(id) => {
                super::super::diagnostics::update(|context| context.measurement = Some(*id));
                crate::plc::diagnostics::event!("PLC_ACK_CHECKPOINT stage=MEASUREMENT_COMMITTED inbox={} measurement_id={id} source={} session={:?} record_seq={} target={:?} idempotent_link=true",
                    row.id, self.config.source_key, row.publication_session, row.record_seq, row.publication_count);
            }
            Err(error) => {
                if self.state.log_changed("MEASUREMENT_COMMIT_ERROR", format!("{}:{error}", row.id)) {
                    crate::plc::diagnostics::event!("PLC_ACK_CHECKPOINT stage=MEASUREMENT_COMMIT_UNCONFIRMED inbox={} operation=InboxService::process error={error} ack_withheld=true retry_same_inbox=true", row.id);
                }
            }
        }
        result.map(|_| ())
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
            let rows: Vec<InboxRecord> = sqlx::query_as("SELECT * FROM plc_inbox WHERE source_id=$1 AND measurement_id IS NULL AND (decode_error IS NULL OR publication_session IS NOT NULL) AND (session_id IS NOT NULL OR publication_session IS NOT NULL) AND (processing_status='FAILED')=$2 ORDER BY (id>$3) DESC,id LIMIT 5")
                .bind(self.source_id).bind(failed).bind(cursor).fetch_all(pool).await.map_err(|e| { crate::plc::diagnostics::sql_failure(concat!(file!(), ":", line!()), &e); e.to_string() })?;
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
        let (failed, waiting): (i64, i64) = sqlx::query_as("SELECT count(*) FILTER (WHERE processing_status='FAILED'),count(*) FILTER (WHERE processing_status<>'FAILED') FROM plc_inbox WHERE source_id=$1 AND measurement_id IS NULL AND (decode_error IS NULL OR publication_session IS NOT NULL) AND (session_id IS NOT NULL OR publication_session IS NOT NULL)")
            .bind(self.source_id).fetch_one(pool).await.map_err(|e| { crate::plc::diagnostics::sql_failure(concat!(file!(), ":", line!()), &e); e.to_string() })?;
        let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM plc_inbox WHERE source_id=$1 AND processing_status='FAILED' AND measurement_id IS NULL AND (decode_error IS NULL OR publication_session IS NOT NULL) ORDER BY id LIMIT 100")
            .bind(self.source_id).fetch_all(pool).await.map_err(|e| { crate::plc::diagnostics::sql_failure(concat!(file!(), ":", line!()), &e); e.to_string() })?;
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

    async fn measurement_committed(pool: &PgPool, id: i64) -> Result<bool, String> {
        let committed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM plc_inbox i JOIN measurements m ON m.id=i.measurement_id WHERE i.id=$1 AND i.processing_status='PROCESSED' AND m.source_id=i.source_id AND m.session_id=i.publication_session AND m.publication_count=i.publication_count AND m.record_seq=i.record_seq)")
            .bind(id).fetch_one(pool).await.map_err(|e| { crate::plc::diagnostics::sql_failure(concat!(file!(), ":", line!()), &e); e.to_string() })?;
        Ok(committed)
    }

    // Log the checkpoint already read by this poll; no extra PLC/SQL reads,
    // no payload dump, and no interpretation of a marker as a successful write.
    fn trace_ack_observation(
        &self,
        stage: &'static str,
        p: &Progress,
        row: &InboxRecord,
        snapshot: &PublishedRecordSnapshot,
    ) {
        self.trace_checkpoint_context(p, Some(row));
        super::super::diagnostics::update(|context| context.phase = stage);
        let c = &snapshot.control;
        let mut comparison = c.clone();
        comparison.heartbeat = 0;
        // Ignore only the independently ticking heartbeat and log timestamp.
        // Compare complete raw buffers in memory, so equal high-level fields
        // never hide a payload change. Nothing here participates in ACK guards.
        let key = format!(
            "endpoint={} unit={} source={} heartbeat_required={} progress={:?} receipt={:?} control={comparison:?} read_raw={:?}",
            self.config.address, self.config.unit_id, self.config.source_key,
            self.config.require_plc_heartbeat_for_ack,
            (p.source_id, &p.session_id, &p.binding, p.accepted, p.last_total,
                p.pending_inbox_id, p.anchor_inbox_id, p.ack_attempted,
                p.done_started, p.done_observed, &p.blocked_reason),
            (row.id, row.measurement_id, &row.processing_status, row.publication_count,
                row.total_count, row.record_seq, &row.word_order, row.buffer_index, &row.raw_registers),
            snapshot.buffer.as_ref().map(|buffer| (buffer.buffer_index, &buffer.raw_registers))
        );
        if !self.state.log_changed(stage, key) {
            return;
        }
        crate::plc::diagnostics::event!(
            "PLC_ACK_OBSERVATION stage={stage} endpoint={} unit_id={} source={} session={} inbox={} measurement_id={:?} processing_status={} target={:?} accepted={} last_total={} pending={:?} anchor={:?} ack_attempted={} done_started={:?} done_observed={} blocked={:?} total={} ready={} pc_count={} pc_done={} heartbeat={} require_heartbeat={} same_receipt={} observed_at={} control={:?} record_seq={} expected_target={:?} snapshot_origin=current_observation",
            self.config.address, self.config.unit_id, self.config.source_key, p.session_id,
            row.id, row.measurement_id, row.processing_status, row.publication_count,
            p.accepted, p.last_total, p.pending_inbox_id, p.anchor_inbox_id,
            p.ack_attempted, p.done_started, p.done_observed, p.blocked_reason,
            c.total_count, c.data_ready, c.pc_read_count, c.pc_read_done, c.heartbeat,
            self.config.require_plc_heartbeat_for_ack, Self::same_receipt(row, snapshot),
            Utc::now().to_rfc3339(), c, row.record_seq, p.accepted.checked_add(1)
        );
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
        self.trace_ack_observation(if p.ack_attempted && p.done_started != Some(false) && c.pc_read_done == 0 {
            "DONE_LOW_OBSERVED"
        } else { "DONE_OBSERVATION" }, p, row, snapshot);
        if !p.ack_attempted || p.done_started == Some(false) {
            return Ok(false);
        }
        if c.pc_read_count == target
            && c.total_count == target
            && c.pc_read_done == 1
            && Self::same_receipt(row, snapshot)
        {
            let observed = sqlx::query("UPDATE plc_acquisition SET done_observed=true WHERE source_id=$1 AND pending_inbox_id=$2 AND ack_attempted")
                .bind(p.source_id).bind(row.id).execute(pool).await.map_err(|e| { crate::plc::diagnostics::sql_failure(concat!(file!(), ":", line!()), &e); e.to_string() })?;
            if self.state.log_changed("DONE_HIGH_UPDATE", format!("{}:{}:{}", row.id, p.done_observed, observed.rows_affected())) {
                crate::plc::diagnostics::event!("PLC_ACK_CHECKPOINT stage=DONE_HIGH_OBSERVED source={} inbox={} target={target} accepted={} rows_affected={} awaiting_done_low=true", self.config.source_key, row.id, p.accepted, observed.rows_affected());
            }
        }
        let complete = row.measurement_id.is_some()
            && p.done_observed
            && c.pc_read_done == 0
            && c.pc_read_count == target
            && ((c.total_count == target && c.data_ready == 0)
                || (target.checked_add(1) == Some(c.total_count)
                    && c.data_ready == 1
                    && snapshot.buffer.is_some()));
        if complete {
            if !Self::measurement_committed(pool, row.id).await? {
                return Err(
                    "MEASUREMENT: linked identity not verified; checkpoint retained".into(),
                );
            }
            let changed = sqlx::query("UPDATE plc_acquisition SET accepted=$3,last_total=GREATEST(last_total,$3),anchor_inbox_id=$2,pending_inbox_id=NULL,ack_attempted=false,done_observed=false,done_started=false WHERE source_id=$1 AND pending_inbox_id=$2 AND accepted=$4 AND session_id=$5 AND binding=$6 AND blocked_reason IS NULL AND ack_attempted AND done_observed AND done_started IS DISTINCT FROM false")
                .bind(p.source_id).bind(row.id).bind(target).bind(p.accepted).bind(&p.session_id).bind(&p.binding).execute(pool).await.map_err(|e| { crate::plc::diagnostics::sql_failure(concat!(file!(), ":", line!()), &e); e.to_string() })?;
            if changed.rows_affected() != 1 {
                crate::plc::diagnostics::event!("PLC_ACK_CHECKPOINT stage=ACK_COMPLETION_CAS_REFUSED expected_rows=1 actual_rows={} accepted_not_confirmed=true", changed.rows_affected());
                return Err("ACK receipt thay đổi; cần đối chiếu.".into());
            }
            crate::plc::diagnostics::event!("PLC_ACK_CHECKPOINT stage=ACK_COMPLETED source={} inbox={} previous_accepted={} accepted={target} pending_cleared=true rows_affected=1", self.config.source_key, row.id, p.accepted);
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
        let reason = if c.pc_read_count != target {
            "COUNT_NOT_TARGET"
        } else if c.pc_read_done != 0 {
            "WAIT_DONE_LOW"
        } else if !p.done_observed {
            "DONE_HIGH_NOT_VERIFIED"
        } else {
            "PUBLICATION_COMPLETION_NOT_VERIFIED"
        };
        Err(format!(
            "ACK_RECONCILIATION_REQUIRED/{reason}: target={target}, accepted={}, Count={}, Done={}, TotalCount={}, DataReady={}, done_started={:?}, done_observed_loaded={}. Chưa đủ bằng chứng Done cao → thấp và publication phù hợp; chỉ quan sát, không gửi lại Count/Done.",
            p.accepted, c.pc_read_count, c.pc_read_done, c.total_count, c.data_ready,
            p.done_started, p.done_observed
        ))
    }

    async fn checked_write_snapshot(
        &self,
        pool: &PgPool,
        row: &InboxRecord,
        expected_count: i32,
        phase: &'static str,
    ) -> Result<(), String> {
        super::super::diagnostics::update(|context| context.phase = phase);
        if self.state.stopping.load(Ordering::SeqCst) {
            return Err("Đang dừng; chưa ghi PLC.".into());
        }
        let snapshot = self.read_snapshot().await.map_err(|error| {
            format!("ACK_FENCE_READ_FAILED: phase={phase}, inbox={}, target={:?}, expected_Count={expected_count}; {error}. Chưa gọi lệnh ghi của giai đoạn này.", row.id, row.publication_count)
        })?;
        crate::plc::diagnostics::event!(
            "PLC_ACK_FENCE stage=READ_BACK phase={phase} source={} inbox={} target={:?} expected_pc_count={expected_count} pc_count={} pc_done={} total={} ready={} same_receipt={} require_heartbeat={} ready_matches={} total_matches={} buffer_present={} count_matches={} done_matches={} observed_at={} control={:?} observation={} write_of_this_phase_not_called=true",
            self.config.source_key, row.id, row.publication_count,
            snapshot.control.pc_read_count, snapshot.control.pc_read_done,
            snapshot.control.total_count, snapshot.control.data_ready,
            Self::same_receipt(row, &snapshot), self.config.require_plc_heartbeat_for_ack,
            snapshot.control.data_ready == 1,
            row.publication_count == Some(snapshot.control.total_count),
            snapshot.buffer.is_some(), snapshot.control.pc_read_count == expected_count,
            snapshot.control.pc_read_done == 0, Utc::now().to_rfc3339(), snapshot.control,
            if phase.starts_with("FC06") { "COUNT_READBACK" } else { "PRE_COUNT_READBACK" }
        );
        if !Self::same_receipt(row, &snapshot)
            || snapshot.control.pc_read_count != expected_count
            || snapshot.control.pc_read_done != 0
        {
            // Diagnostics only: compare the committed WORDs with this read-back.
            // The verified poll baseline matched the same receipt before entering
            // the write path. Include reserved WORDs; never dump the full payload.
            let stored_raw = serde_json::from_value::<Vec<u16>>(row.raw_registers.clone());
            let stored = stored_raw.as_ref().ok().map(Vec::as_slice);
            let actual = snapshot
                .buffer
                .as_ref()
                .map(|buffer| buffer.raw_registers.as_slice());
            let mut differences = 0;
            let mut first_differences = Vec::new();
            if let (Some(before), Some(after)) = (stored, actual) {
                for offset in 0..before.len().max(after.len()) {
                    let old_word = before.get(offset).copied();
                    let new_word = after.get(offset).copied();
                    if old_word != new_word {
                        differences += 1;
                        if first_differences.len() < 8 {
                            let d_address = u16::try_from(
                                usize::from(self.config.control_start)
                                    + as200::map::CONTROL_SIZE
                                    + offset,
                            )
                            .ok();
                            let modbus_address = d_address.and_then(|d| {
                                as200::map::modbus_address(d, self.config.modbus_address_offset)
                            });
                            first_differences.push((
                                offset, d_address, modbus_address, old_word, new_word,
                            ));
                        }
                    }
                }
            }
            crate::plc::diagnostics::event!(
                "PLC_ACK_FENCE stage=SNAPSHOT_MISMATCH reason=SNAPSHOT_CHANGED phase={phase} source={} session={:?} inbox={} measurement_id={:?} processing_status={} receipt_total={:?} receipt_target={:?} receipt_record_seq={} word_order={} receipt_raw_decode_ok={} receipt_words={:?} readback_words={:?} raw_diff_count={:?} diff_columns=offset,d_address,modbus_address,receipt_word,readback_word first_differences={first_differences:?} control={:?}",
                self.config.source_key, row.publication_session, row.id, row.measurement_id,
                row.processing_status, row.total_count, row.publication_count, row.record_seq,
                row.word_order, stored_raw.is_ok(), stored.map(|words| words.len()),
                actual.map(|words| words.len()),
                (stored.is_some() && actual.is_some()).then_some(differences), snapshot.control
            );
            self.block(pool, "SNAPSHOT_CHANGED").await?;
            // These labels explain the existing guard; they grant no write or recovery.
            let reason = if !Self::same_receipt(row, &snapshot) {
                if snapshot.control.data_ready != 1 {
                    "PUBLICATION_NOT_READY"
                } else if row.publication_count != Some(snapshot.control.total_count) {
                    "PUBLICATION_TARGET_MISMATCH"
                } else if snapshot.buffer.is_none() {
                    "PUBLICATION_BUFFER_MISSING"
                } else {
                    "PUBLICATION_RAW_CHANGED"
                }
            } else if snapshot.control.pc_read_count != expected_count {
                "COUNT_READBACK_MISMATCH"
            } else {
                "DONE_NOT_ZERO"
            };
            return Err(format!(
                "SNAPSHOT_CHANGED/{reason}: phase={phase}, inbox={}, target={:?}, Count cần {expected_count} nhưng đọc {}, Done cần 0 nhưng đọc {}, TotalCount={}, DataReady={}. Chưa gọi lệnh ghi của giai đoạn này; giữ receipt/block để đối chiếu, không phát lại ACK.",
                row.id, row.publication_count, snapshot.control.pc_read_count,
                snapshot.control.pc_read_done, snapshot.control.total_count, snapshot.control.data_ready
            ));
        }
        if self.state.stopping.load(Ordering::SeqCst)
            || !self
                .state
                .heartbeat_allows_ack(self.config.require_plc_heartbeat_for_ack)
                .await
        {
            crate::plc::diagnostics::event!("PLC_ACK_FENCE stage=WRITE_POLICY_BLOCKED phase={phase} source={} inbox={} require_heartbeat={} stopping={}", self.config.source_key, row.id, self.config.require_plc_heartbeat_for_ack, self.state.stopping.load(Ordering::SeqCst));
            return Err(
                "Đã giữ raw; đang dừng hoặc heartbeat chưa đáp ứng chính sách ACK, chưa ghi PLC."
                    .into(),
            );
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
        crate::plc::diagnostics::event!("PLC_ACK_CHECKPOINT stage=UNSENT_MARKER_RELEASED source={} inbox={} done_marker={done} transport_not_called=true", self.config.source_key, row.id);
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
        self.trace_checkpoint_context(p, Some(row));
        super::super::diagnostics::update(|context| context.phase = "FC06_PREPARE");
        let writer = self.ack_writer()?;
        let stream = writer.prepare_done().await?;
        self.checked_write_snapshot(pool, row, target, "FC06_BEFORE_MARKER")
            .await?;
        let changed = sqlx::query("UPDATE plc_acquisition SET done_started=true WHERE source_id=$1 AND pending_inbox_id=$2 AND accepted=$3 AND session_id=$4 AND binding=$5 AND blocked_reason IS NULL AND ack_attempted AND done_started=false AND NOT done_observed")
            .bind(p.source_id).bind(row.id).bind(p.accepted).bind(&p.session_id).bind(&p.binding)
            .execute(pool).await.map_err(|e| { crate::plc::diagnostics::sql_failure(concat!(file!(), ":", line!()), &e); e.to_string() })?;
        if changed.rows_affected() != 1 {
            crate::plc::diagnostics::event!("PLC_ACK_CHECKPOINT stage=DONE_MARKER_CAS_REFUSED expected_rows=1 actual_rows={} fc06_not_called=true", changed.rows_affected());
            return Err("Done đã bắt đầu hoặc trạng thái DB đổi; không gửi lặp.".into());
        }
        super::super::diagnostics::update(|context| context.checkpoint.push_str(" done_started_committed=true"));
        crate::plc::diagnostics::event!("PLC_ACK_CHECKPOINT stage=DONE_MARKER_COMMITTED source={} inbox={} target={target} accepted={} done_started=true transport_not_called_yet=true", self.config.source_key, row.id, p.accepted);
        if let Err(error) = self
            .checked_write_snapshot(pool, row, target, "FC06_AFTER_MARKER_BEFORE_CALL")
            .await
        {
            self.release_unsent_marker(pool, p, row, true).await?;
            return Err(error);
        }
        // Any interruption from this call onward leaves done_started=true.
        writer.done_prepared(stream).await?;
        super::super::diagnostics::update(|context| context.phase = "POST_FC06_READBACK");
        let snapshot = self.read_snapshot().await.map_err(|error| {
            format!("ACK_POST_FC06_READ_FAILED: inbox={}, target={target}; FC06 có echo hợp lệ nhưng chưa đọc được bằng chứng hoàn tất: {error}. Giữ intent, không gửi lại Count/Done.", row.id)
        })?;
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
        self.trace_checkpoint_context(&p, None);
        super::super::diagnostics::update(|context| context.phase = "FC03_POLL");
        if p.binding != legacy::binding(&self.config)
            || Some(&p.session_id) != self.config.confirmed_session_id.as_ref()
        {
            crate::plc::diagnostics::event!("PLC_ACK_OBSERVATION stage=ACK_PRECHECK_FAILED guard=SOURCE_SESSION_BINDING expected_session={:?} actual_session={:?} expected_binding={:?} actual_binding={:?} fc16_not_called=true fc06_not_called=true",
                self.config.confirmed_session_id, p.session_id, legacy::binding(&self.config), p.binding);
            return Err("Binding DB thay đổi; không ACK.".into());
        }
        let snapshot = self.read_snapshot().await?;
        let c = &snapshot.control;
        if p.pending_inbox_id.is_none()
            && c.data_ready == 1
            && p.accepted > 0
            && c.total_count == p.accepted
            && p.last_total == p.accepted
        {
            let anchor =
                InboxRepository::find(pool, p.anchor_inbox_id.ok_or("Missing ACK anchor")?).await?;
            let unchanged = anchor.total_count == Some(i64::from(c.total_count))
                && c.pc_read_count == p.accepted
                && c.pc_read_done == 0
                && snapshot.buffer.as_ref().is_some_and(|b| {
                    serde_json::to_value(&b.raw_registers)
                        .is_ok_and(|raw| raw == anchor.raw_registers)
                });
            if !unchanged {
                self.block(pool, "PUBLICATION_CONFLICT").await?;
                return Err("Confirmed publication changed while Ready remained high; no new identity or ACK".into());
            }
            self.show_receipt(&anchor, &p).await?;
            return Ok(());
        }
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
            self.trace_ack_observation("ACK_PRECHECK", &p, row, &snapshot);
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
            if let Some(row) = &pending {
                // A later observation cannot reconstruct the original failing fence.
                self.trace_ack_observation("DURABLE_BLOCK_OBSERVATION", &p, row, &snapshot);
            }
            let mut status = self.state.status.write().await;
            status.pause_reason = Some(reason.clone());
            status.ack_status = "RECONCILIATION_REQUIRED";
            return Err(format!(
                "DB đang giữ trạng thái cần đối chiếu: {reason}; không tự xóa block/đổi epoch."
            ));
        }
        // Do not latch provisional TotalCount values or compare raw with an old
        // anchor while PLC prepares the next record. Existing blocks and pending
        // receipts never take this path; PC-owned Count/Done still must match.
        if pending.is_none() && c.data_ready == 0 {
            if c.pc_read_count < p.accepted {
                self.block(pool, "RESET").await?;
                return Err("Counter PLC giảm; không tự tạo epoch hoặc sửa checkpoint.".into());
            }
            if !Self::idle_control_matches_checkpoint(c, p.accepted, p.last_total)
                || p.ack_attempted
                || p.done_started != Some(false)
                || p.done_observed
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
        let mut row = pending.ok_or("Thiếu receipt của publication; chưa ACK.")?;
        let target = row.publication_count.ok_or("Thiếu định danh công bố.")?;
        if target <= 0 || p.accepted.checked_add(1) != Some(target) {
            self.state.status.write().await.ack_status = "RECONCILIATION_REQUIRED";
            self.trace_ack_observation("PUBLICATION_NOT_NEXT", &p, &row, &snapshot);
            let (reason, detail) = if target == 0 && c.total_count == 0 && !Self::same_receipt(&row, &snapshot) {
                ("PUBLICATION_ZERO_RAW_CHANGED", "Payload counter 0 đã đổi; giữ raw đầu trong DB. Chưa có identity mới để lưu/gửi ACK.")
            } else if target == 0 {
                ("PUBLICATION_COUNTER_ZERO", "Đã lưu observation nhưng target 0 chưa phải publication dương kế tiếp; PROCESSED không có nghĩa được phép ACK. Không lấy RecordSeq thay TotalCount.")
            } else {
                ("PUBLICATION_NOT_NEXT", "Chưa chứng minh publication kế tiếp. Không đổi counter hoặc bỏ dữ liệu để ACK.")
            };
            return Err(format!(
                "{reason}: inbox={}, receipt_target={target}, RecordSeq={}, accepted={}, expected_target={:?}, TotalCount={}, Count={}, Done={}, DataReady={}. {detail}",
                row.id, row.record_seq, p.accepted, p.accepted.checked_add(1),
                c.total_count, c.pc_read_count, c.pc_read_done, c.data_ready
            ));
        }
        if p.ack_attempted && p.done_started != Some(false) {
            return self
                .observe_ack(pool, &p, &row, &snapshot)
                .await
                .map(|_| ());
        }
        // Do not begin/continue a write until the linked Measurement transaction
        // has committed and the receipt has been read back. Lost COMMIT responses
        // retry through the same locked Inbox ID, never a second Measurement.
        if row.measurement_id.is_none() {
            self.process_receipt(pool, &row).await?;
            row = self.receipt(pool, row.id, &p).await?;
            self.show_receipt(&row, &p).await?;
        }
        if row.measurement_id.is_none() {
            return Err("MEASUREMENT: waiting for committed Measurement; ACK withheld".into());
        }
        if !Self::measurement_committed(pool, row.id).await? {
            self.trace_ack_observation("MEASUREMENT_NOT_VERIFIED", &p, &row, &snapshot);
            return Err("MEASUREMENT: committed link/identity not verified; ACK withheld".into());
        }
        if self.config.read_only || !self.config.ack_enabled {
            self.state.status.write().await.ack_status = "DISABLED";
            return Ok(());
        }
        if !Self::same_receipt(&row, &snapshot) {
            self.block(pool, "PUBLICATION_CONFLICT").await?;
            let reason = if c.data_ready == 0 {
                "DataReady về 0 khi receipt còn chờ ACK; PLC phải giữ publication đã công bố"
            } else if c.total_count != target {
                "TotalCount khác publication đã commit"
            } else {
                "Payload khác 240 WORD đã commit"
            };
            return Err(format!(
                "PUBLICATION_CONFLICT: Raw/target khác receipt đã commit; không ACK. {reason} (DataReady={}, TotalCount={}, receipt={target}).",
                c.data_ready, c.total_count
            ));
        }
        // This poll's control is the baseline; raw equality with the durable
        // receipt has just been verified. A later fence logs its own read-back.
        self.trace_ack_observation("POLL_BASELINE_VERIFIED", &p, &row, &snapshot);
        if p.ack_attempted {
            if c.pc_read_count != target || c.pc_read_done != 0 {
                // Repeated observations intentionally retain the uncertain marker.
                self.trace_ack_observation("COUNT_RECONCILE_WAIT", &p, &row, &snapshot);
                let reason = if c.pc_read_count != target {
                    "COUNT_RECONCILE_WAIT"
                } else {
                    "DONE_NOT_ZERO"
                };
                return Err(format!(
                    "{reason}: inbox={}, target={target}, accepted={}, Count={}, Done={}, TotalCount={}, DataReady={}; ack_attempted=true, done_started=false. Chưa xác minh Count/Done để gửi Done lần đầu; tiếp tục quan sát, không phát lại Count.",
                    row.id, p.accepted, c.pc_read_count, c.pc_read_done, c.total_count, c.data_ready
                ));
            }
            self.trace_ack_observation("COUNT_MATCH_FIRST_DONE_ELIGIBLE", &p, &row, &snapshot);
            return self.first_done(pool, &p, &row).await;
        }
        if c.pc_read_count != p.accepted || c.pc_read_done != 0 {
            return Err("Count/Done khác điểm bắt đầu đã xác nhận; chưa gửi ACK.".into());
        }
        if self.state.stopping.load(Ordering::SeqCst) {
            return Err("Đang dừng trước Count.".into());
        }
        let writer = self.ack_writer()?;
        self.trace_ack_observation("FIRST_COUNT_PREPARATION", &p, &row, &snapshot);
        let stream = writer.prepare_count().await?;
        self.checked_write_snapshot(pool, &row, p.accepted, "FC16_BEFORE_MARKER")
            .await?;
        let changed = sqlx::query("UPDATE plc_acquisition SET ack_attempted=true WHERE source_id=$1 AND pending_inbox_id=$2 AND accepted=$3 AND session_id=$4 AND binding=$5 AND blocked_reason IS NULL AND NOT ack_attempted AND done_started=false AND NOT done_observed")
            .bind(p.source_id).bind(row.id).bind(p.accepted).bind(&p.session_id).bind(&p.binding)
            .execute(pool).await.map_err(|e| { crate::plc::diagnostics::sql_failure(concat!(file!(), ":", line!()), &e); e.to_string() })?;
        if changed.rows_affected() != 1 {
            crate::plc::diagnostics::event!("PLC_ACK_CHECKPOINT stage=COUNT_MARKER_CAS_REFUSED expected_rows=1 actual_rows={} fc16_not_called=true", changed.rows_affected());
            return Err("ACK đã bắt đầu hoặc trạng thái đổi; không gửi lặp.".into());
        }
        p.ack_attempted = true;
        self.trace_checkpoint_context(&p, Some(&row));
        crate::plc::diagnostics::event!("PLC_ACK_CHECKPOINT stage=COUNT_MARKER_COMMITTED source={} inbox={} target={target} accepted={} ack_attempted=true done_started={:?} transport_not_called_yet=true", self.config.source_key, row.id, p.accepted, p.done_started);
        if let Err(error) = self
            .checked_write_snapshot(pool, &row, p.accepted, "FC16_AFTER_MARKER_BEFORE_CALL")
            .await
        {
            self.release_unsent_marker(pool, &p, &row, false).await?;
            return Err(error);
        }
        writer.count_prepared(target, stream).await?;
        crate::plc::diagnostics::event!("PLC_ACK_CHECKPOINT stage=COUNT_ECHO_OK source={} inbox={} target={target} accepted={} readback_required=true checkpoint_confirmed=false", self.config.source_key, row.id, p.accepted);
        self.first_done(pool, &p, &row).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::test_support::receipt;

    fn control() -> PlcControl {
        let mut words = vec![0; 20];
        words[0] = 1;
        words[1] = 240;
        words[2] = 2;
        words[8] = 1;
        decode::control(&words, as200::WordOrder::LowWordFirst).unwrap()
    }

    #[test]
    fn idle_preparation_keeps_pc_handshake_and_checkpoint_requirements() {
        let mut c = control();
        for total in [0, 1, 2, 50] {
            c.total_count = total;
            assert!(Engine::idle_control_matches_checkpoint(&c, 1, 1));
            // Previously observed publication history is never discarded.
            assert!(!Engine::idle_control_matches_checkpoint(&c, 1, 2));
        }
        for count in [0, 2] {
            c.pc_read_count = count;
            assert!(!Engine::idle_control_matches_checkpoint(&c, 1, 1));
        }
        c.pc_read_count = 1;
        c.pc_read_done = 1;
        assert!(!Engine::idle_control_matches_checkpoint(&c, 1, 1));
        c.pc_read_done = 0;
        c.data_ready = 1;
        assert!(!Engine::idle_control_matches_checkpoint(&c, 1, 1));
    }

    #[test]
    fn processed_receipt_still_requires_ready_identity_and_unchanged_raw() {
        let mut row = receipt(2);
        row.measurement_id = Some(10);
        row.processing_status = "PROCESSED".into();
        let mut c = control();
        c.data_ready = 1;
        c.total_count = 2;
        let snapshot = PublishedRecordSnapshot {
            control: c,
            buffer: Some(as200::reader::BufferSnapshot {
                buffer_index: 0,
                raw_registers: serde_json::from_value(row.raw_registers.clone()).unwrap(),
                record: None,
            }),
            diagnostics: Vec::new(),
        };
        assert!(Engine::same_receipt(&row, &snapshot));
        let mut changed = snapshot.clone();
        changed.control.data_ready = 0;
        changed.buffer = None;
        assert!(!Engine::same_receipt(&row, &changed));
        let mut changed = snapshot.clone();
        changed.control.total_count = 3;
        assert!(!Engine::same_receipt(&row, &changed));
        // Even a reserved WORD differs: do not ACK an overwritten receipt.
        let mut changed = snapshot;
        changed.buffer.as_mut().unwrap().raw_registers[131] ^= 1;
        assert!(!Engine::same_receipt(&row, &changed));
    }
}
