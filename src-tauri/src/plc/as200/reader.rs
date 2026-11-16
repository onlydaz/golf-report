use super::{As200Config, As200Error, decode, map, model::*};
use std::{future::Future, net::SocketAddr};
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
            let socket: SocketAddr = self.config.address.parse().map_err(|_| {
                As200Error::Transport(format!(
                    "Địa chỉ PLC phải là IP:port dạng số: {}",
                    self.config.address
                ))
            })?;
            let ctx = tcp::connect_slave(socket, Slave(self.config.unit_id))
                .await
                .map_err(|e| As200Error::Transport(e.to_string()))?;
            self.ctx = Some(ctx);
        }
        let ctx = self.ctx.as_mut().expect("ctx vừa được tạo");
        // Gọi `call` trực tiếp thay vì `Reader::read_holding_registers`: hàm đó chỉ
        // `debug_assert` số WORD (panic ở debug build nếu PLC trả thiếu dữ liệu).
        let response = ctx
            .call(Request::ReadHoldingRegisters(address, count))
            .await
            .map_err(|e| As200Error::Transport(e.to_string()))?
            .map_err(|e| As200Error::Transport(format!("PLC trả exception FC03: {e}")))?;
        let Response::ReadHoldingRegisters(words) = response else {
            return Err(As200Error::Transport(format!(
                "FC03 nhận response không đúng loại: {response:?}"
            )));
        };
        if words.len() != usize::from(count) {
            return Err(As200Error::Transport(format!(
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
        // Sau lỗi/timeout, socket có thể chứa response dở. Hủy kết nối, nối lại ở lần gọi sau.
        if result.is_err() {
            self.ctx = None;
        }
        result
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnostic {
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
/// Không mang nghĩa dữ liệu mới, đã lưu hoặc được phép ACK; caller phải kiểm tra diagnostics.
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
            let block = self
                .transport
                .read_holding_registers(address + result.len() as u16, block_size as u16)
                .await?;
            if block.len() != block_size {
                return Err(As200Error::Transport("FC03 trả thiếu WORD".into()));
            }
            result.extend(block);
        }
        Ok(result)
    }
    pub async fn read_control(&mut self) -> Result<PlcControl, As200Error> {
        let raw = self
            .read_range(map::CONTROL_BASE, map::CONTROL_SIZE)
            .await?;
        decode::control(&raw, self.config.word_order)
    }
    /// Raw Bản ghi 1; caller must surround this with stable control/raw checks.
    pub async fn read_published_words(&mut self) -> Result<Vec<u16>, As200Error> {
        self.read_range(
            map::CONTROL_BASE + map::CONTROL_SIZE as u16,
            map::RECORD_SIZE,
        )
        .await
    }
    /// Đọc riêng D25020..D25259 khi DataReady=1, không đọc buffer index 1.
    /// Dùng lại decoder/validation, không dùng hoặc cập nhật tracker hai buffer.
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
            return Err(As200Error::Unstable);
        }
        if raw != self.read_published_words().await? {
            return Err(As200Error::Unstable);
        }
        let after = self.read_control().await?;
        if !same_control(&before, &after) {
            return Err(As200Error::Unstable);
        }

        let mut diagnostics = Vec::new();
        let record = if raw[map::record::STATUS] & 1 == 0 {
            diagnostics.push(Diagnostic::InvalidRecord {
                buffer_index: 0,
                error: "DataReady=1 nhưng Bản ghi 1 chưa có Status bit0 hợp lệ".into(),
            });
            None
        } else {
            match decode::record(&raw, 0, self.config.word_order) {
                Ok(record) => {
                    if let Err(error) = record.validate_metadata(Some(i64::from(after.total_count)))
                    {
                        diagnostics.push(Diagnostic::InvalidRecord {
                            buffer_index: 0,
                            error,
                        });
                    }
                    if !record.complete() {
                        diagnostics.push(Diagnostic::IncompleteRecord {
                            buffer_index: 0,
                            record_seq: record.record_seq,
                        });
                    } else if let Err(error) = record.validate_workflow() {
                        diagnostics.push(Diagnostic::InvalidRecord {
                            buffer_index: 0,
                            error,
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
