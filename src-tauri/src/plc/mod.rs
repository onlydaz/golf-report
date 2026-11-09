pub mod config;
mod decode;
mod protocol;
mod register_map;
mod storage;
#[cfg(test)]
mod tests;

use crate::{database::DatabaseState, models::CreateMeasurementRequest};
use config::PlcConfig;
use register_map::{COMPLETED, SEQUENCE};
use serde::Serialize;
use std::sync::Arc;
use tauri::State;
use tokio::sync::Mutex;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlcStatus {
    connection: String,
    address: String,
    unit: u8,
    phase: String,
    message: String,
    product_code: Option<String>,
    sequence_id: Option<u16>,
    data: Option<CreateMeasurementRequest>,
    measurement_id: Option<i64>,
    busy: bool,
}
struct Session {
    product: Option<String>,
    baseline: Option<u16>,
    // Giữ nguyên mã sản phẩm và thời gian kể cả khi inbox tạm thời không ghi được.
    captured: Option<(Option<i64>, u16, CreateMeasurementRequest)>,
    processing: Option<(i64, i32)>,
}
#[derive(Clone)]
pub struct PlcState {
    config: PlcConfig,
    session: Arc<Mutex<Session>>,
    status: Arc<Mutex<PlcStatus>>,
}
impl Default for PlcState {
    fn default() -> Self {
        let config = PlcConfig::default();
        Self {
            status: Arc::new(Mutex::new(PlcStatus {
                connection: "CONNECTING".into(),
                address: config.address.clone(),
                unit: config.unit,
                phase: "WAITING".into(),
                message: "Nhập mã sản phẩm để bắt đầu ghi nhận.".into(),
                product_code: None,
                sequence_id: None,
                data: None,
                measurement_id: None,
                busy: false,
            })),
            config,
            session: Arc::new(Mutex::new(Session {
                product: None,
                baseline: None,
                captured: None,
                processing: None,
            })),
        }
    }
}
impl PlcState {
    async fn phase(&self, phase: &str, message: &str, busy: bool) {
        let mut status = self.status.lock().await;
        status.phase = phase.into();
        status.message = message.into();
        status.busy = busy;
    }
    pub async fn run(self, database: DatabaseState) {
        loop {
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(12), self.tick(&database))
                    .await;
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    let busy = self.status.lock().await.busy;
                    self.phase("ERROR", &error, busy).await;
                }
                Err(_) => {
                    let session = self.session.lock().await;
                    let busy = session.captured.is_some() || session.processing.is_some();
                    self.phase("ERROR", "Hết thời gian xử lý. Sẽ tự động thử lại; không đổi mã sản phẩm khi còn dữ liệu chờ.", busy).await;
                }
            }
            tokio::time::sleep(self.config.poll).await;
        }
    }
    async fn tick(&self, database: &DatabaseState) -> Result<(), String> {
        let mut session = self.session.lock().await;
        // Chụp trạng thái kết nối độc lập với PostgreSQL, tự kết nối lại mỗi lượt đọc.
        let snapshot = protocol::snapshot(&self.config).await;
        {
            let mut status = self.status.lock().await;
            if snapshot.is_ok() && status.connection == "DISCONNECTED" && !status.busy {
                status.phase = "WAITING".into();
                status.message = "PLC đã kết nối lại. Đang chờ phép đo mới.".into();
            }
            status.connection = if snapshot.is_ok() {
                "CONNECTED"
            } else {
                "DISCONNECTED"
            }
            .into();
        }
        // Giữ dữ liệu và thời gian nhận ngay cả khi PostgreSQL mất kết nối.
        if session.captured.is_none() && session.processing.is_none() {
            if let Ok(Some(raw)) = &snapshot {
                if raw[COMPLETED] == 1 && session.baseline != Some(raw[SEQUENCE]) {
                    if let Some(product) = &session.product {
                        self.phase("RECEIVING", "Đang nhận và kiểm tra kết quả PLC...", true)
                            .await;
                        match decode::decode(raw, product) {
                            Ok(request) => {
                                let mut status = self.status.lock().await;
                                status.data = Some(request.clone());
                                status.sequence_id = Some(raw[SEQUENCE]);
                                status.measurement_id = None;
                                session.captured = Some((None, raw[SEQUENCE], request));
                            }
                            Err(error) => {
                                self.phase("ERROR", &error, false).await;
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }
        let pool = database.pool().await?;
        if let Some((generation, sequence, request)) = session.captured.clone() {
            let generation = match generation {
                Some(generation) => generation,
                None => storage::observe(&pool, &self.config.source, sequence).await?,
            };
            session.captured = Some((Some(generation), sequence, request.clone()));
            storage::enqueue(&pool, &self.config.source, generation, sequence, &request).await?;
            session.processing = Some((generation, i32::from(sequence)));
            session.baseline = Some(sequence);
            session.captured = None;
            return Ok(());
        }
        if session.processing.is_none() {
            session.processing = storage::pending(&pool, &self.config.source)
                .await?
                .map(|(generation, sequence, _)| (generation, sequence));
        }
        if let Some((generation, sequence)) = session.processing {
            self.phase("SAVING", "Đang lưu PostgreSQL...", true).await;
            let (id, request) =
                storage::save(&pool, &self.config.source, generation, sequence).await?;
            let mut status = self.status.lock().await;
            status.data = Some(request);
            status.sequence_id = Some(sequence as u16);
            status.measurement_id = Some(id);
            status.phase = "SAVED".into();
            status.message = format!("Đã lưu thành công. Mã bản ghi: {id}.");
            status.busy = false;
            session.processing = None;
            return Ok(());
        }
        let Some(raw) = snapshot? else {
            return Ok(());
        };
        if raw[COMPLETED] > 1 {
            return Err("Giá trị completed không hợp lệ.".into());
        }
        storage::observe(&pool, &self.config.source, raw[SEQUENCE]).await?;
        if raw[COMPLETED] != 1 {
            return Ok(());
        }
        if session.product.is_none() {
            session.baseline = Some(raw[SEQUENCE]);
        }
        Ok(())
    }
}

#[tauri::command]
pub async fn get_plc_status(state: State<'_, PlcState>) -> Result<PlcStatus, String> {
    Ok(state.status.lock().await.clone())
}

#[tauri::command]
pub async fn start_plc_capture(
    product_code: String,
    state: State<'_, PlcState>,
    database: State<'_, DatabaseState>,
) -> Result<(), String> {
    tokio::time::timeout(
        std::time::Duration::from_secs(12),
        begin_capture(&product_code, &state, &database),
    )
    .await
    .map_err(|_| "Hết thời gian xác nhận mã sản phẩm. Vui lòng thử lại.".to_string())?
}

async fn begin_capture(
    product_code: &str,
    state: &PlcState,
    database: &DatabaseState,
) -> Result<(), String> {
    let product = product_code.trim();
    if product.is_empty() || product.chars().count() > 50 {
        return Err("Mã sản phẩm phải có từ 1 đến 50 ký tự.".into());
    }
    let mut session = state.session.lock().await;
    if session.captured.is_some() || session.processing.is_some() {
        return Err("Đang xử lý kết quả; vui lòng chờ lưu thành công.".into());
    }
    let pool = database.pool().await?;
    if storage::pending(&pool, &state.config.source)
        .await?
        .is_some()
    {
        return Err("Còn kết quả chờ lưu; không thể đổi mã sản phẩm.".into());
    }
    // Đọc trực tiếp lúc xác nhận: tuyệt đối không dùng snapshot UI cũ làm mốc.
    let raw = protocol::snapshot(&state.config)
        .await?
        .ok_or("PLC đang cập nhật; hãy xác nhận lại mã sản phẩm.")?;
    if raw[COMPLETED] > 1 {
        return Err("Giá trị completed không hợp lệ.".into());
    }
    storage::observe(&pool, &state.config.source, raw[1]).await?;
    session.baseline = Some(raw[1]);
    session.product = Some(product.into());
    let mut status = state.status.lock().await;
    status.connection = "CONNECTED".into();
    status.product_code = Some(product.into());
    status.phase = "WAITING".into();
    status.message = "Đã xác nhận mã sản phẩm. Đang chờ phép đo mới từ PLC.".into();
    status.data = None;
    status.measurement_id = None;
    status.sequence_id = None;
    status.busy = false;
    Ok(())
}
