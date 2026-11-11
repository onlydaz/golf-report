use super::{As200Config, As200Error, decode, map, model::*};
use std::{collections::HashMap, future::Future};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

/// Transport chỉ đọc, dễ thay bằng bộ nhớ giả trong kiểm thử. Không có API FC06/FC16.
pub trait HoldingRegisterReader {
    fn read_holding_registers(
        &mut self,
        address: u16,
        count: u16,
    ) -> impl Future<Output = Result<Vec<u16>, As200Error>> + Send;
}

pub struct ModbusTcpReader {
    config: As200Config,
    stream: Option<TcpStream>,
    transaction: u16,
}
impl ModbusTcpReader {
    pub fn new(config: As200Config) -> Self {
        Self {
            config,
            stream: None,
            transaction: 0,
        }
    }

    async fn exchange(&mut self, address: u16, count: u16) -> Result<Vec<u16>, As200Error> {
        let transport = |error: std::io::Error| As200Error::Transport(error.to_string());
        if self.stream.is_none() {
            self.stream = Some(
                TcpStream::connect(&self.config.address)
                    .await
                    .map_err(transport)?,
            );
        }
        self.transaction = self.transaction.wrapping_add(1);
        let [th, tl] = self.transaction.to_be_bytes();
        let [ah, al] = address.to_be_bytes();
        let [ch, cl] = count.to_be_bytes();
        let stream = self.stream.as_mut().unwrap();
        stream
            .write_all(&[th, tl, 0, 0, 0, 6, self.config.unit_id, 3, ah, al, ch, cl])
            .await
            .map_err(transport)?;
        let mut header = [0; 7];
        stream.read_exact(&mut header).await.map_err(transport)?;
        let length = u16::from_be_bytes([header[4], header[5]]) as usize;
        if header[..4] != [th, tl, 0, 0]
            || header[6] != self.config.unit_id
            || !(3..=254).contains(&length)
        {
            return Err(As200Error::Transport(
                "MBAP sai transaction/protocol/unit/length".into(),
            ));
        }
        let mut body = vec![0; length - 1];
        stream.read_exact(&mut body).await.map_err(transport)?;
        if body[0] == 0x83 {
            return Err(As200Error::Transport(format!(
                "PLC trả exception FC03: {}",
                body[1]
            )));
        }
        if body[0] != 3
            || body.len() != 2 + usize::from(count) * 2
            || usize::from(body[1]) != usize::from(count) * 2
        {
            return Err(As200Error::Transport(
                "FC03 sai function/byte count/kích thước dữ liệu".into(),
            ));
        }
        Ok(body[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect())
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
            return Err(As200Error::InvalidData(
                "FC03 cần 1–125 WORD và vùng địa chỉ hợp lệ".into(),
            ));
        }
        let duration = self.config.timeout;
        let result = timeout(duration, self.exchange(address, count))
            .await
            .unwrap_or(Err(As200Error::Timeout));
        // Sau timeout, socket có thể chứa response dở. Hủy socket, nối lại ở lần gọi sau.
        if result.is_err() {
            self.stream = None;
        }
        result
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnostic {
    TrackerRejected {
        error: String,
        session_unconfirmed: bool,
    },
    InvalidRecord {
        buffer_index: usize,
        error: String,
    },
    BufferOverrunRisk {
        created_since_last_poll: i32,
        capacity: usize,
    },
    UnaccountedRecords {
        created_since_last_poll: i32,
        available_new_records: usize,
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
#[derive(Debug, Clone)]
pub struct PollReport {
    pub control: PlcControl,
    pub buffers: Vec<BufferSnapshot>,
    // Chỉ thông báo phát hiện trong RAM, KHÔNG mang nghĩa đã lưu/đã ACK.
    // Thứ tự theo buffer; không khẳng định thứ tự thời gian của RecordSeq.
    pub new_records: Vec<PlcStickRecord>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Default)]
pub struct RecordTracker {
    pub(super) total_count: Option<i32>,
    recent: HashMap<i32, Vec<u16>>,
    session_unconfirmed: bool,
}
impl RecordTracker {
    fn identity_error(
        &mut self,
        control: &PlcControl,
        buffers: &[BufferSnapshot],
        order: WordOrder,
    ) -> Option<As200Error> {
        if self.session_unconfirmed
            || control.total_count < 0
            || self.total_count.is_some_and(|p| control.total_count < p)
        {
            self.session_unconfirmed = true;
            return Some(As200Error::SessionUnconfirmed);
        }
        let mut sequences = std::collections::HashSet::new();
        for b in buffers
            .iter()
            .filter(|b| b.raw_registers[map::record::STATUS] & 1 != 0)
        {
            let seq = decode::dint32(&b.raw_registers, map::record::SEQUENCE, order).ok()?;
            if !sequences.insert(seq) {
                return Some(As200Error::InvalidData(
                    "Duplicate RecordSeq in stable slots".into(),
                ));
            }
            if self
                .recent
                .get(&seq)
                .is_some_and(|old| old != &b.raw_registers)
            {
                return Some(As200Error::InvalidData(
                    "Raw changed for the same RecordSeq".into(),
                ));
            }
        }
        None
    }
    /// Caller chỉ gọi sau khi đã xác nhận phiên/reset PLC bằng thông tin ngoài register map.
    /// Snapshot đầu tiếp theo là mốc mới, không tự ghép counter của hai phiên.
    pub fn reset_after_confirmed_session_change(&mut self) {
        *self = Self::default();
    }

    fn observe(
        &mut self,
        control: &PlcControl,
        buffers: &[BufferSnapshot],
    ) -> Result<(Vec<PlcStickRecord>, Vec<Diagnostic>), As200Error> {
        if self.session_unconfirmed {
            return Err(As200Error::SessionUnconfirmed);
        }
        if control.total_count < 0
            || self
                .total_count
                .is_some_and(|previous| control.total_count < previous)
        {
            self.session_unconfirmed = true;
            return Err(As200Error::SessionUnconfirmed);
        }
        let mut diagnostics = Vec::new();
        let mut current = HashMap::new();
        let mut candidates = Vec::new();
        for record in buffers.iter().filter_map(|buffer| buffer.record.as_ref()) {
            if current
                .insert(record.record_seq, record.raw_registers.clone())
                .is_some()
            {
                return Err(As200Error::InvalidData(
                    "Hai slot đang hợp lệ có cùng RecordSeq".into(),
                ));
            }
            if let Some(previous) = self.recent.get(&record.record_seq) {
                if previous != &record.raw_registers {
                    return Err(As200Error::InvalidData(format!(
                        "Nội dung RecordSeq {} đổi mà sequence không đổi",
                        record.record_seq
                    )));
                }
            }
            if !record.complete() {
                diagnostics.push(Diagnostic::IncompleteRecord {
                    buffer_index: record.buffer_index,
                    record_seq: record.record_seq,
                });
            } else if !self.recent.contains_key(&record.record_seq) {
                candidates.push(record.clone());
            }
        }
        // Không đánh dấu đã quan sát nếu bản ghi còn chưa đầy đủ.
        if !diagnostics.is_empty() {
            return Ok((Vec::new(), diagnostics));
        }
        if let Some(previous) = self.total_count {
            let delta = control.total_count - previous;
            if candidates.len() > delta as usize {
                return Err(As200Error::InvalidData(
                    "RecordSeq mới không tương thích mức tăng TotalCount".into(),
                ));
            }
            if delta > map::BUFFER_COUNT as i32 {
                diagnostics.push(Diagnostic::BufferOverrunRisk {
                    created_since_last_poll: delta,
                    capacity: map::BUFFER_COUNT,
                });
            }
            if delta as usize > candidates.len() {
                diagnostics.push(Diagnostic::UnaccountedRecords {
                    created_since_last_poll: delta,
                    available_new_records: candidates.len(),
                });
            }
        } else {
            // Mốc ban đầu: chỉ hiển thị snapshot hiện tại, không tự nhận nó là phép đo mới.
            candidates.clear();
        }
        self.total_count = Some(control.total_count);
        self.recent = current;
        Ok((candidates, diagnostics))
    }
}

pub struct As200Reader<T = ModbusTcpReader> {
    config: As200Config,
    pub(super) transport: T,
    pub tracker: RecordTracker,
}
impl As200Reader<ModbusTcpReader> {
    pub fn new(config: As200Config) -> Self {
        Self::with_transport(config.clone(), ModbusTcpReader::new(config))
    }
}
impl<T: HoldingRegisterReader> As200Reader<T> {
    pub fn with_transport(config: As200Config, transport: T) -> Self {
        Self {
            config,
            transport,
            tracker: RecordTracker::default(),
        }
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
    /// Có thể dùng riêng để kiểm tra từng buffer; đây chưa phải snapshot nhất quán.
    pub async fn read_record_words(&mut self, index: usize) -> Result<Vec<u16>, As200Error> {
        let base = map::record_base(index)
            .ok_or_else(|| As200Error::InvalidData("Buffer index phải là 0 hoặc 1".into()))?;
        self.read_range(base, map::RECORD_SIZE).await
    }
    pub async fn poll(&mut self) -> Result<PollReport, As200Error> {
        let before = self.read_control().await?;
        let mut first = Vec::new();
        for index in 0..map::BUFFER_COUNT {
            first.push(self.read_record_words(index).await?);
        }
        let middle = self.read_control().await?;
        if !before.same_buffer_state(&middle) {
            return Err(As200Error::Unstable);
        }
        // Đọc lại toàn bộ record để bắt cả trường hợp PLC chưa cập nhật Control khi đang ghi slot.
        for (index, previous) in first.iter().enumerate() {
            if previous != &self.read_record_words(index).await? {
                return Err(As200Error::Unstable);
            }
        }
        let after = self.read_control().await?;
        if !before.same_buffer_state(&after) {
            return Err(As200Error::Unstable);
        }
        let mut buffers = Vec::new();
        let mut decode_errors = Vec::new();
        for (index, words) in first.into_iter().enumerate() {
            let record = if words[map::record::STATUS] & 1 != 0 {
                match decode::record(&words, index, self.config.word_order) {
                    Ok(record) => {
                        if let Err(error) =
                            record.validate_metadata(Some(i64::from(after.total_count)))
                        {
                            decode_errors.push(Diagnostic::InvalidRecord {
                                buffer_index: index,
                                error,
                            });
                        }
                        // Validation trước tracker: payload sai nghiệp vụ không được
                        // che mất bởi lỗi "sequence/raw thay đổi" ở tracker.
                        if !record.complete() {
                            decode_errors.push(Diagnostic::IncompleteRecord {
                                buffer_index: index,
                                record_seq: record.record_seq,
                            });
                        } else {
                            if let Err(error) = record.validate_workflow() {
                                decode_errors.push(Diagnostic::InvalidRecord {
                                    buffer_index: index,
                                    error,
                                });
                            }
                        }
                        Some(record)
                    }
                    Err(error) => {
                        decode_errors.push(Diagnostic::InvalidRecord {
                            buffer_index: index,
                            error: error.to_string(),
                        });
                        None
                    }
                }
            } else {
                None
            };
            buffers.push(BufferSnapshot {
                buffer_index: index,
                raw_registers: words,
                record,
            });
        }
        // Slot lỗi vẫn trả đủ raw; không tiến tracker qua dữ liệu chưa hiểu được.
        // Worker có thể lưu slot tốt còn lại và quarantine slot lỗi trong cùng snapshot.
        if let Some(error) = self
            .tracker
            .identity_error(&after, &buffers, self.config.word_order)
        {
            decode_errors.push(Diagnostic::TrackerRejected {
                session_unconfirmed: error == As200Error::SessionUnconfirmed,
                error: error.to_string(),
            });
        }
        let (new_records, diagnostics) = if decode_errors.is_empty() {
            match self.tracker.observe(&after, &buffers) {
                Ok(value) => value,
                Err(error) => (
                    Vec::new(),
                    vec![Diagnostic::TrackerRejected {
                        session_unconfirmed: error == As200Error::SessionUnconfirmed,
                        error: error.to_string(),
                    }],
                ),
            }
        } else {
            (Vec::new(), decode_errors)
        };
        Ok(PollReport {
            control: after,
            buffers,
            new_records,
            diagnostics,
        })
    }
    /// Caller có thể gọi định kỳ; lỗi được log và lần gọi sau tự thử kết nối lại.
    pub async fn poll_logged(&mut self) -> Result<PollReport, As200Error> {
        let result = self.poll().await;
        match &result {
            Err(error) => eprintln!(
                "[AS200 {} unit={}] {error}",
                self.config.address, self.config.unit_id
            ),
            Ok(report) => {
                for diagnostic in &report.diagnostics {
                    eprintln!("[AS200] {diagnostic:?}");
                }
                for record in &report.new_records {
                    eprintln!(
                        "[AS200] buffer={} RecordSeq={} StickNo={} TotalCount={}",
                        record.buffer_index,
                        record.record_seq,
                        record.stick_no,
                        report.control.total_count
                    );
                }
            }
        }
        result
    }
}
