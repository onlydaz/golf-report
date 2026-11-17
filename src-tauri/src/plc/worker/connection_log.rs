//! Connection telemetry only. Never reads or clears measurement/ACK journals.
use super::*;

#[derive(Default)]
pub(super) struct ConnectionLog {
    path: Option<PathBuf>,
    events: Vec<ConnectionEvent>,
    current: Option<(String, String)>,
}

impl ConnectionLog {
    fn persist(&self, events: &[ConnectionEvent]) -> Result<(), String> {
        let path = self.path.as_ref().ok_or("Nhật ký kết nối chưa sẵn sàng.")?;
        let bytes = serde_json::to_vec(events)
            .map_err(|_| "Không mã hóa được nhật ký kết nối.".to_string())?;
        let tmp = path.with_extension(format!("{}.tmp", Utc::now().timestamp_nanos_opt().unwrap_or_default()));
        let result = (|| {
            let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&tmp, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result.map_err(|_: std::io::Error| "Không lưu được nhật ký kết nối. Kiểm tra quyền ghi thư mục ứng dụng; lịch sử đo và ACK không bị thay đổi.".into())
    }
}

impl WorkerState {
    pub(super) async fn load_connection_log(&self, directory: &std::path::Path) {
        let mut log = self.connection_log.lock().await;
        if log.path.is_some() {
            return;
        }
        let path = directory.join("plc-connection-events.json");
        let loaded = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Vec<ConnectionEvent>>(&bytes)
                .map_err(|_| "Nhật ký kết nối bị lỗi định dạng; giữ nguyên file để kiểm tra.".to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(_) => Err("Không đọc được nhật ký kết nối. Kiểm tra quyền truy cập file.".into()),
        };
        let mut status = self.status.write().await;
        match loaded {
            Ok(events) => {
                log.path = Some(path);
                log.events = events;
                status.connection_events = log.events.clone();
                status.connection_log_error = None;
            }
            Err(error) => status.connection_log_error = Some(error),
        }
    }

    pub(in crate::plc) async fn record_connection(&self, kind: &str, endpoint: &str) {
        let mut log = self.connection_log.lock().await;
        if log.path.is_none() {
            return; // Keep the precise load error; never overwrite an unreadable/corrupt file.
        }
        let phase = match kind {
            "CONNECTED" | "RECONNECTED" => "ONLINE",
            "DISCONNECTED" | "FAILED" => "OFFLINE",
            _ => "STOPPED",
        };
        let current = (phase.to_string(), endpoint.to_string());
        if log.current.as_ref() == Some(&current) {
            return;
        }
        let label = match kind {
            "CONNECTED" => "Kết nối PLC thành công",
            "RECONNECTED" => "Kết nối lại PLC thành công",
            "DISCONNECTED" => "Mất kết nối PLC",
            "FAILED" => "Kết nối PLC thất bại",
            _ => "Ngắt kết nối PLC thủ công",
        };
        let mut events = log.events.clone();
        events.push(ConnectionEvent {
            at: Utc::now(),
            kind: kind.into(),
            endpoint: endpoint.into(),
            message: format!("{label} ({endpoint})"),
        });
        match log.persist(&events) {
            Ok(()) => {
                log.events = events;
                log.current = Some(current);
                let mut status = self.status.write().await;
                status.connection_events = log.events.clone();
                status.connection_log_error = None;
            }
            Err(error) => self.status.write().await.connection_log_error = Some(error),
        }
    }

    pub(in crate::plc) async fn clear_connection_log(&self) -> Result<(), String> {
        let mut log = self.connection_log.lock().await;
        log.persist(&[])?;
        log.events.clear();
        // Retain the current phase: polling must not recreate the deleted event.
        let mut status = self.status.write().await;
        status.connection_events.clear();
        status.connection_log_error = None;
        Ok(())
    }
}
