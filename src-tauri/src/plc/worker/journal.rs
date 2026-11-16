//! Journal v4 only. Reject incompatible history before deserializing or writing.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Delivery {
    pub(super) input: InboxInput,
    pub(super) inbox_id: Option<i64>,
    pub(super) terminal: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct QuarantinedRecord {
    #[serde(default)]
    pub(super) control: Option<super::as200::model::PlcControl>,
    pub(super) source_key: String,
    pub(super) session_id: Option<String>,
    pub(super) address: String,
    pub(super) unit_id: u8,
    pub(super) modbus_address_offset: i32,
    pub(super) word_order: crate::models::StoredWordOrder,
    pub(super) buffer_index: usize,
    pub(super) record_seq: Option<i32>,
    pub(super) total_count: i32,
    pub(super) received_at: DateTime<Utc>,
    pub(super) raw_registers: Vec<u16>,
    pub(super) error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualRetryRequest {
    pub inbox_id: i64,
    pub state: String,
    pub error: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Journal {
    #[serde(default)]
    pub(super) manual_retries: Vec<ManualRetryRequest>,
    #[serde(default)]
    pub(super) retries: Vec<retry::RetryEntry>,
    #[serde(default)]
    pub(super) quarantine: Vec<QuarantinedRecord>,
    #[serde(default)]
    pub(super) previous_session_warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) published_ack: Option<PublishedAckProgress>,
    pub(super) version: u8,
    pub(super) source_key: String,
    pub(super) session_id: Option<String>,
    pub(super) last_total: Option<i32>,
    pub(super) pause: Option<String>,
    pub(super) warnings: Vec<String>,
    pub(super) deliveries: Vec<Delivery>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PublishedAckProgress {
    pub(super) binding: String,
    pub(super) accepted: i32,
    /// Last confirmed, processed receipt: retained even after compacting deliveries.
    pub(super) confirmed_receipt: Option<Delivery>,
    /// ACK_CONFIRMED is retained until a subsequent prepared intent replaces it.
    pub(super) intent: Option<PublishedAckIntent>,
}

impl Journal {
    pub(super) fn validate(&self, config: &SimulatorConfig) -> Result<(), String> {
        if self.version != 4
            || self.source_key != config.source_key
            || self.session_id != config.confirmed_session_id
            || self.session_id.is_none()
        {
            return Err("Journal v4 sai SourceKey/Session/version; không tự sửa lịch sử.".into());
        }
        let progress = self
            .published_ack
            .as_ref()
            .ok_or("Journal v4 thiếu ACK progress.")?;
        if progress.binding != Engine::published_binding(config) || progress.accepted < 0 {
            return Err(
                "ACK Bản ghi 1: sai binding/checkpoint; không đổi nguồn hoặc phiên để vượt lỗi."
                    .into(),
            );
        }
        match &progress.confirmed_receipt {
            None if progress.accepted == 0 => {}
            Some(receipt)
                if progress.accepted > 0
                    && receipt.input.record_seq == progress.accepted
                    && receipt.input.confirmed_session_id == self.session_id
                    && receipt.input.buffer_index == 0
                    && receipt.inbox_id.is_some()
                    && receipt.input.word_order == config.word_order
                    && receipt.input.raw_registers.len() == super::as200::map::RECORD_SIZE => {}
            _ => {
                return Err(
                    "ACK Bản ghi 1: thiếu receipt checkpoint lịch sử; cần đối chiếu.".into(),
                );
            }
        }
        let mut seen = std::collections::HashSet::new();
        if self.deliveries.iter().any(|d| {
            d.input.confirmed_session_id != self.session_id
                || d.input.record_seq <= progress.accepted
                || !seen.insert(d.input.record_seq)
        }) {
            return Err("Journal v4 có receipt trùng, sai phiên hoặc nằm trước checkpoint.".into());
        }
        if let Some(intent) = &progress.intent {
            let receipt = if intent.phase == PublishedAckPhase::AckConfirmed {
                progress.confirmed_receipt.as_ref()
            } else {
                self.deliveries
                    .iter()
                    .find(|d| d.input.record_seq == intent.target)
            }
            .ok_or("ACK Bản ghi 1: intent thiếu receipt.")?;
            let expected = if intent.phase == PublishedAckPhase::AckConfirmed {
                Some(progress.accepted)
            } else {
                progress.accepted.checked_add(1)
            };
            if expected != Some(intent.target)
                || receipt.inbox_id != Some(intent.inbox_id)
                || receipt.input.record_seq != intent.target
                || receipt.input.raw_registers != intent.raw_registers
                || (intent.phase == PublishedAckPhase::AckConfirmed && !intent.observed_done_high)
                || (intent.observed_done_high
                    && !matches!(
                        intent.phase,
                        PublishedAckPhase::DoneWriteUncertain | PublishedAckPhase::AckConfirmed
                    ))
            {
                return Err("ACK Bản ghi 1: intent/checkpoint/raw không khớp.".into());
            }
        }
        Ok(())
    }

    pub(super) fn load(path: &std::path::Path, config: &SimulatorConfig) -> Result<Self, String> {
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("Không đọc được Journal: {e}"))?;
        let mut value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("Journal lỗi; không ghi đè: {e}"))?;
        // In particular, never deserialize a legacy ACK intent into a default v4 checkpoint.
        if value.get("version").and_then(|v| v.as_u64()) != Some(4) {
            return Err("Journal cũ không thuộc Published ACK v4; cần kỹ thuật đối chiếu receipt/checkpoint và ACK intent. Không tự chuyển đổi hoặc ghi đè.".into());
        }
        if value
            .get("ack")
            .is_some_and(|a| a != &serde_json::json!({"binding":"", "accepted":0, "intent":null}))
            || value.get("session_request").is_some_and(|v| !v.is_null())
            || value
                .get("session_archives")
                .is_some_and(|v| v.as_array().is_none_or(|v| !v.is_empty()))
        {
            return Err(
                "Journal có lịch sử ACK/phiên của cơ chế cũ; giữ nguyên để đối chiếu.".into(),
            );
        }
        // v4 from Prompt 3C included these empty legacy markers. Only empty
        // markers may be omitted; actual history/unknown fields are never dropped.
        if let Some(object) = value.as_object_mut() {
            for marker in ["ack", "session_request", "session_archives"] {
                object.remove(marker);
            }
        }
        let journal: Self = serde_json::from_value(value)
            .map_err(|e| format!("Journal v4 lỗi; không ghi đè: {e}"))?;
        journal.validate(config)?;
        Ok(journal)
    }

    pub(super) fn pending_ack(&self) -> bool {
        self.published_ack
            .as_ref()
            .and_then(|p| p.intent.as_ref())
            .is_some_and(|i| i.phase != PublishedAckPhase::AckConfirmed)
    }
}

impl Engine {
    pub(super) fn published_binding(config: &SimulatorConfig) -> String {
        format!(
            "published-ack-v1|{}|{}|{}|{}|{:?}|{}|{:?}|as200",
            config.mode,
            config.address,
            config.unit_id,
            config.source_key,
            config.confirmed_session_id,
            config.modbus_address_offset,
            config.word_order
        )
    }

    pub(super) fn open(
        config: SimulatorConfig,
        path: PathBuf,
        state: WorkerState,
    ) -> Result<Arc<Self>, String> {
        config.validate()?;
        if !config.ack_enabled
            || !config.allow_plc_connection
            || !simulator_config::ack_scope_allows(&config)
            || config.pc_heartbeat_enabled
            || config.allow_plc_heartbeat_write
            || config.confirmed_session_id.is_none()
            || config.session_transition.is_some()
        {
            return Err("Published ACK cần cấu hình AS200 đã xác nhận, SourceKey/Session rõ ràng và tắt ghi PC Heartbeat. Không tự chuyển phiên hoặc cấp quyền ghi.".into());
        }
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
        lock.try_lock()
            .map_err(|e| format!("Journal đang được Worker khác sử dụng: {e}"))?;
        let journal = if path.exists() {
            Journal::load(&path, &config)?
        } else {
            Journal {
                version: 4,
                source_key: config.source_key.clone(),
                session_id: config.confirmed_session_id.clone(),
                last_total: None,
                pause: None,
                warnings: Vec::new(),
                deliveries: Vec::new(),
                manual_retries: Vec::new(),
                retries: Vec::new(),
                quarantine: Vec::new(),
                previous_session_warnings: Vec::new(),
                published_ack: Some(PublishedAckProgress {
                    binding: Self::published_binding(&config),
                    accepted: 0,
                    confirmed_receipt: None,
                    intent: None,
                }),
            }
        };
        journal.validate(&config)?;
        Ok(Arc::new(Self {
            config,
            path,
            journal: Mutex::new(journal),
            state,
            _lock: lock,
            published_gate: Mutex::new(()),
        }))
    }

    pub(super) async fn verify_open(&self, pool: &PgPool) -> Result<(), String> {
        if !self.path.exists() {
            let seen: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id WHERE s.source_key=$1) OR EXISTS(SELECT 1 FROM measurements m JOIN plc_sources s ON s.id=m.source_id WHERE s.source_key=$1)")
                .bind(&self.config.source_key).fetch_one(pool).await.map_err(|e| e.to_string())?;
            if seen {
                return Err("Nguồn đã có lịch sử nhưng thiếu Journal v4; không tự nhập checkpoint hoặc tạo phiên khác.".into());
            }
        }
        self.verify_published_history(pool).await?;
        let anchor = self
            .journal
            .lock()
            .await
            .published_ack
            .as_ref()
            .and_then(|p| p.confirmed_receipt.clone());
        if let Some(receipt) = anchor {
            let row = recovery::processed_receipt(&self.config, &receipt, pool).await?;
            self.row_status(&row).await?;
        }
        self.publish_journal().await
    }

    pub(super) fn persist(&self, journal: &Journal) -> Result<(), String> {
        let tmp = self.path.with_extension("tmp");
        let mut file = File::create(&tmp).map_err(|e| e.to_string())?;
        file.write_all(&serde_json::to_vec(journal).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        std::fs::rename(tmp,&self.path).map_err(|e|format!("Không lưu được journal: {e}. Dữ liệu chưa vào inbox có nguy cơ mất khi ứng dụng đóng."))
    }

    pub(super) fn warn(journal: &mut Journal, message: String) {
        if !journal.warnings.contains(&message) {
            journal.warnings.push(message);
        }
    }

    pub(super) fn persist_published_transition(
        &self,
        journal: &mut Journal,
        next: Journal,
    ) -> Result<(), String> {
        next.validate(&self.config)?;
        if let Err(error) = self.persist(&next) {
            journal.pause = Some("JOURNAL_WRITE_FAILED".into());
            Self::warn(journal, error.clone());
            return Err(error);
        }
        *journal = next;
        Ok(())
    }
}
