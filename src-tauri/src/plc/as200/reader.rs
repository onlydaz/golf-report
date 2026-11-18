use super::{As200Config, As200Error, decode, map, model::*};
use std::future::Future;
use tokio::time::timeout;
use tokio_modbus::{
    Request, Response, Slave,
    client::{Client, Context, tcp},
};

/// Transport chỉ đọc, dễ thay bằng bộ nhớ giả trong kiểm thử. Không có API FC06/FC16.
pub trait HoldingRegisterReader {
    fn read_holding_registers(
        &mut self,
        address: u16,
        count: u16,
    ) -> impl Future<Output = Result<Vec<u16>, As200Error>> + Send;
}

/// FC03 qua `tokio-modbus`: thư viện lo MBAP (transaction/unit/length) và
/// đối chiếu function code; lớp này giữ timeout, giới hạn WORD và nối lại.
pub struct ModbusTcpReader {
    config: As200Config,
    ctx: Option<Context>,
}
impl ModbusTcpReader {
    pub fn new(config: As200Config) -> Self {
        Self { config, ctx: None }
    }

    async fn exchange(&mut self, address: u16, count: u16) -> Result<Vec<u16>, As200Error> {
        if self.ctx.is_none() {
            let stream = super::connect_tcp(
                &self.config.address,
                self.config.local_bind_ip,
                self.config.timeout,
            )
            .await?;
            let ctx = tcp::attach_slave(stream, Slave(self.config.unit_id));
            self.ctx = Some(ctx);
        }
        let ctx = self.ctx.as_mut().expect("ctx vừa được tạo");
        // Gọi `call` trực tiếp thay vì `Reader::read_holding_registers`: hàm đó chỉ
        // `debug_assert` số WORD (panic ở debug build nếu PLC trả thiếu dữ liệu).
        let response = ctx
            .call(Request::ReadHoldingRegisters(address, count))
            .await
            .map_err(|e| match e {
                tokio_modbus::Error::Protocol(_) => As200Error::Protocol(e.to_string()),
                tokio_modbus::Error::Transport(ref io)
                    if io.kind() == std::io::ErrorKind::InvalidData =>
                {
                    As200Error::Protocol(e.to_string())
                }
                _ => As200Error::Transport(e.to_string()),
            })?
            .map_err(|e| As200Error::Protocol(format!("PLC trả exception FC03: {e}")))?;
        let Response::ReadHoldingRegisters(words) = response else {
            return Err(As200Error::Protocol(format!(
                "FC03 nhận response không đúng loại: {response:?}"
            )));
        };
        if words.len() != usize::from(count) {
            return Err(As200Error::Protocol(format!(
                "FC03 sai kích thước dữ liệu: cần {count} WORD, nhận {}",
                words.len()
            )));
        }
        Ok(words)
    }
}
impl HoldingRegisterReader for ModbusTcpReader {
    async fn read_holding_registers(
        &mut self,
        address: u16,
        count: u16,
    ) -> Result<Vec<u16>, As200Error> {
        if count == 0
            || usize::from(count) > map::FC03_LIMIT
            || u32::from(address) + u32::from(count) > 65536
        {
            return Err(As200Error::InvalidData(format!(
                "FC03 cần 1–{} WORD và vùng địa chỉ hợp lệ",
                map::FC03_LIMIT
            )));
        }
        let duration = self.config.timeout;
        let result = timeout(duration, self.exchange(address, count))
            .await
            .unwrap_or(Err(As200Error::Timeout));
        if matches!(&result, Err(As200Error::Timeout)) {
            eprintln!(
                "PLC_READ state=TIMEOUT endpoint={} stage={} timeout_ms={}",
                self.config.address,
                if self.ctx.is_some() {
                    "FC03"
                } else {
                    "TCP_CONNECT"
                },
                duration.as_millis()
            );
        }
        // Sau lỗi/timeout, socket có thể chứa response dở. Hủy kết nối, nối lại ở lần gọi sau.
        if result.is_err() {
            self.ctx = None;
        }
        result
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnostic {
    /// Payload toàn 0: giữ raw; không suy diễn thành measurement.
    ZeroPayload,
    InvalidRecord {
        buffer_index: usize,
        error: String,
    },
    IncompleteRecord {
        buffer_index: usize,
        record_seq: i32,
    },
}
#[derive(Debug, Clone)]
pub struct BufferSnapshot {
    pub buffer_index: usize,
    pub raw_registers: Vec<u16>,
    // Slot không có Status bit 0 được giữ raw, không suy diễn record rỗng thành dữ liệu hợp lệ.
    pub record: Option<PlcStickRecord>,
}
/// Snapshot ổn định của vùng công bố Bản ghi 1.
/// Diagnostics chỉ quyết định khả năng giải mã measurement; ACK cần receipt raw và handshake.
#[derive(Debug, Clone)]
pub struct PublishedRecordSnapshot {
    pub control: PlcControl,
    /// None khi DataReady=0 đã được đọc lại ổn định; khi đó không đọc vùng bản ghi.
    pub buffer: Option<BufferSnapshot>,
    pub diagnostics: Vec<Diagnostic>,
}
pub struct As200Reader<T = ModbusTcpReader> {
    config: As200Config,
    pub(super) transport: T,
}
impl As200Reader<ModbusTcpReader> {
    pub fn new(config: As200Config) -> Self {
        Self::with_transport(config.clone(), ModbusTcpReader::new(config))
    }
}
impl<T: HoldingRegisterReader> As200Reader<T> {
    pub fn with_transport(config: As200Config, transport: T) -> Self {
        Self { config, transport }
    }
    async fn read_range(&mut self, d_address: u16, count: usize) -> Result<Vec<u16>, As200Error> {
        let address = map::modbus_address(d_address, self.config.modbus_address_offset)
            .ok_or_else(|| {
                As200Error::InvalidData("Offset Modbus làm địa chỉ vượt phạm vi u16".into())
            })?;
        if usize::from(address) + count > 65536 {
            return Err(As200Error::InvalidData(
                "Vùng đọc vượt địa chỉ Modbus".into(),
            ));
        }
        let mut result = Vec::with_capacity(count);
        while result.len() < count {
            let block_size = (count - result.len()).min(map::FC03_LIMIT);
            let request_address = address + result.len() as u16;
            let block = self
                .transport
                .read_holding_registers(request_address, block_size as u16)
                .await
                .map_err(|error| {
                    eprintln!(
                        "PLC_FC03 state=ERROR endpoint={} unit_id={} d_start={} modbus_start={} offset={} words={} reason={error}",
                        self.config.address, self.config.unit_id,
                        usize::from(d_address) + result.len(), request_address,
                        self.config.modbus_address_offset, block_size
                    );
                    error
                })?;
            if block.len() != block_size {
                eprintln!(
                    "PLC_FC03 state=SHORT_RESPONSE endpoint={} unit_id={} modbus_start={} expected_words={} received_words={}",
                    self.config.address,
                    self.config.unit_id,
                    request_address,
                    block_size,
                    block.len()
                );
                return Err(As200Error::Protocol("FC03 trả thiếu WORD".into()));
            }
            result.extend(block);
        }
        Ok(result)
    }
    pub async fn read_control(&mut self) -> Result<PlcControl, As200Error> {
        if self.config.record_size != map::RECORD_SIZE
            || self.config.buffer_count != map::BUFFER_COUNT
            || usize::from(self.config.control_start)
                + map::CONTROL_SIZE
                + self.config.record_size * self.config.buffer_count
                > 65536
        {
            return Err(As200Error::InvalidData(
                "Profile AS200 chỉ hỗ trợ 2 buffer × 240 WORD và vùng nhớ hợp lệ".into(),
            ));
        }
        let raw = self
            .read_range(self.config.control_start, map::CONTROL_SIZE)
            .await?;
        decode::control(&raw, self.config.word_order).map_err(|error| {
            eprintln!(
                "PLC_CONTROL state=INVALID endpoint={} unit_id={} d_start={} modbus_start={:?} offset={} word_order={:?} words={raw:?} reason={error}",
                self.config.address, self.config.unit_id, self.config.control_start,
                map::modbus_address(self.config.control_start, self.config.modbus_address_offset),
                self.config.modbus_address_offset, self.config.word_order
            );
            error
        })
    }
    /// Theo xác nhận kỹ thuật PLC: PC chỉ đọc Bản ghi 1 khi DataReady=1;
    /// PLC chịu trách nhiệm công bố và quản lý toàn bộ buffer vật lý.
    /// Không chọn vùng theo WriteIdx/LastIdx. Caller phải kiểm tra control/raw ổn định.
    /// Xác nhận vùng đọc này không thay thế việc nghiệm thu trình tự ACK Count/Done.
    pub async fn read_published_words(&mut self) -> Result<Vec<u16>, As200Error> {
        self.read_range(
            self.config
                .control_start
                .checked_add(map::CONTROL_SIZE as u16)
                .ok_or_else(|| As200Error::InvalidData("Địa chỉ Bản ghi 1 vượt phạm vi".into()))?,
            self.config.record_size,
        )
        .await
    }
    /// Đọc riêng D25020..D25259 khi DataReady=1, không đọc buffer index 1.
    /// PLC quản lý buffer vật lý; PC không ghi WriteIdx/LastIdx hoặc chuyển buffer.
    /// DataReady có thể giữ mức 1 và RecordSeq có thể nhỏ hơn TotalCount.
    /// Đây chỉ là snapshot quan sát; không xác nhận nguồn/phiên, lưu dữ liệu hay ACK.
    pub async fn poll_published_record(&mut self) -> Result<PublishedRecordSnapshot, As200Error> {
        // Heartbeat có thể tăng độc lập; Count/Done phải ổn định trong lần đọc này.
        let same_control = |a: &PlcControl, b: &PlcControl| {
            a.same_buffer_state(b)
                && a.pc_read_count == b.pc_read_count
                && a.pc_read_done == b.pc_read_done
        };
        let before = self.read_control().await?;
        if before.data_ready == 0 {
            let after = self.read_control().await?;
            if !same_control(&before, &after) {
                eprintln!(
                    "PLC_CONTROL state=UNSTABLE stage=IDLE endpoint={} before={before:?} after={after:?}",
                    self.config.address
                );
                return Err(As200Error::Unstable);
            }
            return Ok(PublishedRecordSnapshot {
                control: after,
                buffer: None,
                diagnostics: Vec::new(),
            });
        }

        let raw = self.read_published_words().await?;
        let middle = self.read_control().await?;
        if !same_control(&before, &middle) {
            eprintln!(
                "PLC_CONTROL state=UNSTABLE stage=AFTER_PAYLOAD endpoint={} before={before:?} after={middle:?}",
                self.config.address
            );
            return Err(As200Error::Unstable);
        }
        if raw != self.read_published_words().await? {
            eprintln!(
                "PLC_SNAPSHOT state=UNSTABLE endpoint={} reason=published_payload_changed",
                self.config.address
            );
            return Err(As200Error::Unstable);
        }
        let after = self.read_control().await?;
        if !same_control(&before, &after) {
            eprintln!(
                "PLC_CONTROL state=UNSTABLE stage=AFTER_RECHECK endpoint={} before={before:?} after={after:?}",
                self.config.address
            );
            return Err(As200Error::Unstable);
        }

        let mut diagnostics = Vec::new();
        let record = if raw.iter().all(|word| *word == 0) {
            diagnostics.push(Diagnostic::ZeroPayload);
            None // Keep all 240 words. Version/Status/sequence are not invented.
        } else if raw[map::record::STATUS] & 1 == 0 {
            diagnostics.push(Diagnostic::InvalidRecord {
                buffer_index: 0,
                error: "DataReady=1 nhưng Bản ghi 1 chưa có Status bit0 hợp lệ".into(),
            });
            None
        } else {
            match decode::record(&raw, 0, self.config.word_order) {
                Ok(record) => {
                    if !record.complete() {
                        diagnostics.push(Diagnostic::IncompleteRecord {
                            buffer_index: 0,
                            record_seq: record.record_seq,
                        });
                    }
                    Some(record)
                }
                Err(error) => {
                    diagnostics.push(Diagnostic::InvalidRecord {
                        buffer_index: 0,
                        error: error.to_string(),
                    });
                    None
                }
            }
        };
        Ok(PublishedRecordSnapshot {
            control: after,
            buffer: Some(BufferSnapshot {
                buffer_index: 0,
                raw_registers: raw,
                record,
            }),
            diagnostics,
        })
    }
}
