//! Whitelisted AS200 ACK transport with protocol-specific acceptance evidence.
use super::As200Config;
use super::{
    WordOrder, map,
    reader::{As200Reader, PollReport},
};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AckProgress {
    pub binding: String,
    pub accepted: i32,
    pub intent: Option<AckIntent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckIntent {
    pub target: i32,
    pub records: Vec<(usize, Vec<u16>)>,
    #[serde(default)]
    pub submitted_at_ms: Option<i64>,
    #[serde(default)]
    pub observed_done_high: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckPhase {
    NotSent,
    OutcomeUnknown,
    DoneObserved,
}
impl AckIntent {
    // Tương thích journal cũ: submitted không đồng nghĩa lệnh đã tới PLC.
    pub fn phase(&self) -> AckPhase {
        if self.observed_done_high && self.submitted_at_ms.is_some() {
            AckPhase::DoneObserved
        } else if self.submitted_at_ms.is_some() {
            AckPhase::OutcomeUnknown
        } else {
            AckPhase::NotSent
        }
    }
}

/// Excel: PLC xóa bit0 sau ACK. Chỉ nhận bằng chứng khi vẫn nhận diện được
/// đúng raw đã lưu (mọi bit khác không đổi), không suy từ slot rỗng/ghi đè.
fn exact_records_released(intent: &AckIntent, report: &PollReport) -> bool {
    !intent.records.is_empty()
        && intent.records.iter().all(|(index, raw)| {
            raw.len() == map::RECORD_SIZE
                && raw[map::record::STATUS] & 1 != 0
                && report.buffers.iter().any(|b| {
                    b.buffer_index == *index
                        && b.raw_registers.len() == raw.len()
                        && b.raw_registers[map::record::STATUS] & 1 == 0
                        && raw
                            .iter()
                            .zip(&b.raw_registers)
                            .enumerate()
                            .all(|(i, (a, b))| {
                                if i == map::record::STATUS {
                                    a & !1 == b & !1
                                } else {
                                    a == b
                                }
                            })
                })
        })
}
pub fn same_records(intent: &AckIntent, report: &PollReport) -> bool {
    intent.records.iter().all(|(index, raw)| {
        report
            .buffers
            .iter()
            .any(|b| b.buffer_index == *index && b.raw_registers == *raw)
    })
}
pub fn accepted(intent: &AckIntent, report: &PollReport) -> bool {
    let c = &report.control;
    c.total_count >= intent.target
        && c.pc_read_count == intent.target
        && c.pc_read_done == 0
        && c.data_ready == i16::from(c.total_count > intent.target)
        // simulator-v1 khóa tạo record từ lúc nhận Count tới lúc chấp nhận Done.
        // Total tăng sau target vì vậy cũng chứng minh lần ACK trước đã được xử lý.
        && (c.total_count > intent.target || intent.records.iter().all(|(index,raw)| {
            report.buffers.iter().any(|b| b.buffer_index==*index && b.raw_registers.len()==raw.len()
                && raw.iter().zip(&b.raw_registers).enumerate().all(|(i,(a,b))|
                    if i==map::record::STATUS { a & !1 == b & !1 } else { a==b }))
        }))
}

pub fn accepted_as200(intent: &AckIntent, report: &PollReport) -> bool {
    let c = &report.control;
    // Không dùng write response làm evidence. Ngoài cạnh 1->0, Excel định nghĩa
    // PLC xóa valid sau ACK: có thể phục hồi missed pulse nếu đúng raw còn nguyên.
    intent.submitted_at_ms.is_some()
        && (intent.observed_done_high
            || (c.total_count == intent.target && exact_records_released(intent, report)))
        && report.diagnostics.is_empty()
        && c.pc_read_count == intent.target
        && c.pc_read_done == 0
        && c.total_count >= intent.target
        && c.data_ready == i16::from(c.total_count > intent.target)
}

/// Chỉ constructor này cấp quyền ghi, dựa trên mode/protocol và opt-in rõ ràng.
pub struct AckWriter {
    cfg: As200Config,
}
/// Capability riêng: chỉ FC06 D25010, không cấp quyền ACK.
pub struct HeartbeatWriter {
    cfg: As200Config,
}
impl HeartbeatWriter {
    pub fn new(
        cfg: As200Config,
        mode: &str,
        enabled: bool,
        allow_connection: bool,
        allow_write: bool,
    ) -> Result<Self, String> {
        let address: std::net::SocketAddr = cfg
            .address
            .parse()
            .map_err(|_| "Heartbeat cần numeric IP:port")?;
        if !enabled
            || !(1..=247).contains(&cfg.unit_id)
            || address.port() == 0
            || !match mode {
                "simulator" => address.ip().is_loopback(),
                "plc" | "modbus" => allow_connection && allow_write,
                _ => false,
            }
        {
            return Err("PC_Heartbeat write capability disabled".into());
        }
        Ok(Self { cfg })
    }
    pub async fn write(&self, value: u16) -> Result<(), String> {
        if value > 32767 {
            return Err("PC_Heartbeat ngoài 0..32767".into());
        }
        let address = map::modbus_address(
            map::CONTROL_BASE + map::control::PC_HEARTBEAT as u16,
            self.cfg.modbus_address_offset,
        )
        .ok_or("Heartbeat address overflow")?;
        let [a, b] = address.to_be_bytes();
        let [hi, lo] = value.to_be_bytes();
        let pdu = vec![6, a, b, hi, lo];
        // Dùng chung framing/timeout/echo checks, không gọi constructor hoặc API ACK.
        AckWriter {
            cfg: self.cfg.clone(),
        }
        .exchange_on(pdu.clone(), pdu, None)
        .await
    }
}
impl AckWriter {
    pub fn new(
        cfg: As200Config,
        mode: &str,
        enabled: bool,
        protocol: Option<&str>,
        allow_plc_connection: bool,
    ) -> Result<Self, String> {
        let address: std::net::SocketAddr = cfg
            .address
            .parse()
            .map_err(|_| "ACK requires numeric IP:port")?;
        let permitted = match mode {
            "simulator" => {
                address.ip().is_loopback() && matches!(protocol, Some("simulator-v1" | "as200"))
            }
            "plc" | "modbus" => allow_plc_connection && protocol == Some("as200"),
            _ => false,
        };
        if !enabled || !permitted || address.port() == 0 {
            return Err("ACK write capability disabled".into());
        }
        Ok(Self { cfg })
    }
    // Không có API địa chỉ tùy ý: FC16 chỉ D25008..09, FC06 chỉ D25012=1.
    async fn exchange_on(
        &self,
        pdu: Vec<u8>,
        expected: Vec<u8>,
        prepared: Option<TcpStream>,
    ) -> Result<(), String> {
        let operation = async {
            let mut stream = match prepared {
                Some(stream) => stream,
                None => TcpStream::connect(&self.cfg.address)
                    .await
                    .map_err(|e| e.to_string())?,
            };
            let length = (pdu.len() + 1) as u16;
            let mut request = vec![
                0,
                1,
                0,
                0,
                (length >> 8) as u8,
                length as u8,
                self.cfg.unit_id,
            ];
            request.extend(&pdu);
            stream
                .write_all(&request)
                .await
                .map_err(|e| e.to_string())?;
            let mut header = [0u8; 7];
            stream
                .read_exact(&mut header)
                .await
                .map_err(|e| e.to_string())?;
            let size = u16::from_be_bytes([header[4], header[5]]) as usize;
            if header[..4] != [0, 1, 0, 0]
                || header[6] != self.cfg.unit_id
                || !(3..=254).contains(&size)
            {
                return Err("ACK invalid MBAP".into());
            }
            let mut body = vec![0; size - 1];
            stream
                .read_exact(&mut body)
                .await
                .map_err(|e| e.to_string())?;
            if body != expected {
                return Err(format!("ACK refused/malformed response: {body:?}"));
            }
            Ok(())
        };
        tokio::time::timeout(self.cfg.timeout, operation)
            .await
            .map_err(|_| "ACK timeout".to_string())?
    }
    pub async fn count(&self, count: i32) -> Result<(), String> {
        if count <= 0 {
            return Err("ACK invalid count".into());
        }
        let address = map::modbus_address(
            map::CONTROL_BASE + map::control::PC_READ_COUNT as u16,
            self.cfg.modbus_address_offset,
        )
        .ok_or("ACK address overflow")?;
        let [a, b] = address.to_be_bytes();
        let bits = count as u32;
        let mut words = [(bits >> 16) as u16, bits as u16];
        if matches!(self.cfg.word_order, WordOrder::LowWordFirst) {
            words.reverse();
        }
        let mut pdu = vec![16, a, b, 0, 2, 4];
        for word in words {
            pdu.extend(word.to_be_bytes());
        }
        self.exchange_on(pdu, vec![16, a, b, 0, 2], None).await
    }
    pub async fn done(&self) -> Result<(), String> {
        let stream = self.prepare_done().await?;
        self.done_prepared(stream).await
    }
    /// Mở socket chưa gửi byte nào: lỗi connect không làm intent thành "đã gửi".
    pub async fn prepare_done(&self) -> Result<TcpStream, String> {
        tokio::time::timeout(self.cfg.timeout, TcpStream::connect(&self.cfg.address))
            .await
            .map_err(|_| "ACK connect timeout trước Done".to_string())?
            .map_err(|e| e.to_string())
    }
    pub async fn done_prepared(&self, stream: TcpStream) -> Result<(), String> {
        let address = map::modbus_address(
            map::CONTROL_BASE + map::control::PC_READ_DONE as u16,
            self.cfg.modbus_address_offset,
        )
        .ok_or("ACK address overflow")?;
        let [a, b] = address.to_be_bytes();
        let pdu = vec![6, a, b, 0, 1];
        self.exchange_on(pdu.clone(), pdu, Some(stream)).await
    }
    pub async fn verify_snapshot(&self, intent: &AckIntent) -> Result<PollReport, String> {
        let mut reader = As200Reader::new(self.cfg.clone());
        let report = reader.poll().await.map_err(|e| e.to_string())?;
        if !report.diagnostics.is_empty()
            || !same_records(intent, &report)
            || report.control.total_count < intent.target
        {
            return Err("ACK buffer/session changed".into());
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[tokio::test]
    async fn heartbeat_capability_has_separate_opt_in_and_only_writes_d25010() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let cfg = As200Config {
            address: listener.local_addr().unwrap().to_string(),
            unit_id: 1,
            modbus_address_offset: -1,
            word_order: WordOrder::LowWordFirst,
            timeout: Duration::from_millis(500),
        };
        for (mode, enabled, connection, allow) in [
            ("plc", false, true, true),
            ("plc", true, true, false),
            ("plc", true, false, true),
            ("simulator", false, false, false),
        ] {
            assert!(HeartbeatWriter::new(cfg.clone(), mode, enabled, connection, allow).is_err());
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
        let writer = HeartbeatWriter::new(cfg, "plc", true, true, true).unwrap();
        let server = tokio::spawn(async move {
            for bytes in [[0x7f, 0xff], [0, 0]] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 12];
                stream.read_exact(&mut request).await.unwrap();
                assert_eq!(&request[7..10], &[6, 0x61, 0xb1]); // D25010 - 1, không phải Count/Done.
                assert_eq!(&request[10..12], &bytes);
                stream.write_all(&request).await.unwrap();
            }
        });
        writer.write(32767).await.unwrap();
        writer.write(0).await.unwrap();
        assert!(writer.write(32768).await.is_err());
        server.await.unwrap();
    }
    #[tokio::test]
    async fn fc16_word_order_offset_and_fc06_echo_are_checked() {
        for order in [WordOrder::LowWordFirst, WordOrder::HighWordFirst] {
            for mode in ["simulator", "plc"] {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let address = listener.local_addr().unwrap().to_string();
                let server = tokio::spawn(async move {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut request = [0; 17];
                    stream.read_exact(&mut request).await.unwrap();
                    assert_eq!(&request[7..13], &[16, 97, 175, 0, 2, 4]); // D25008 - 1
                    let expected = if order == WordOrder::LowWordFirst {
                        [0, 2, 0, 1]
                    } else {
                        [0, 1, 0, 2]
                    };
                    assert_eq!(&request[13..], &expected);
                    stream
                        .write_all(&[0, 1, 0, 0, 0, 6, 1, 16, 97, 175, 0, 2])
                        .await
                        .unwrap();
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut request = [0; 12];
                    stream.read_exact(&mut request).await.unwrap();
                    assert_eq!(&request[7..], &[6, 97, 179, 0, 1]); // D25012 - 1
                    stream.write_all(&request).await.unwrap();
                });
                let cfg = As200Config {
                    address,
                    unit_id: 1,
                    modbus_address_offset: -1,
                    word_order: order,
                    timeout: Duration::from_millis(500),
                };
                assert!(
                    AckWriter::new(cfg.clone(), "plc", true, Some("simulator-v1"), false).is_err()
                );
                assert!(AckWriter::new(cfg.clone(), "plc", false, Some("as200"), true).is_err());
                assert!(AckWriter::new(cfg.clone(), "plc", true, Some("as200"), false).is_err());
                let writer = AckWriter::new(
                    cfg,
                    mode,
                    true,
                    Some(if mode == "plc" {
                        "as200"
                    } else {
                        "simulator-v1"
                    }),
                    mode == "plc",
                )
                .unwrap();
                writer.count(65538).await.unwrap();
                writer.done().await.unwrap();
                server.await.unwrap();
            }
        }
    }
    #[tokio::test]
    async fn missing_or_bad_response_never_counts_as_success() {
        for fault in ["disconnect", "timeout", "bad-mbap"] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap().to_string();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 12];
                stream.read_exact(&mut request).await.unwrap();
                match fault {
                    "timeout" => tokio::time::sleep(Duration::from_millis(100)).await,
                    "bad-mbap" => {
                        request[1] = 99;
                        stream.write_all(&request).await.unwrap();
                    }
                    _ => {}
                }
            });
            let cfg = As200Config {
                address,
                unit_id: 1,
                modbus_address_offset: 0,
                word_order: WordOrder::LowWordFirst,
                timeout: Duration::from_millis(50),
            };
            let writer =
                AckWriter::new(cfg, "simulator", true, Some("simulator-v1"), false).unwrap();
            assert!(writer.done().await.is_err());
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn prepared_done_does_not_send_until_explicitly_consumed() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let cfg = As200Config {
            address: listener.local_addr().unwrap().to_string(),
            unit_id: 1,
            modbus_address_offset: 0,
            word_order: WordOrder::LowWordFirst,
            timeout: Duration::from_millis(100),
        };
        let writer = AckWriter::new(cfg, "plc", true, Some("as200"), true).unwrap();
        let prepared = writer.prepare_done().await.unwrap();
        let (mut server, _) = listener.accept().await.unwrap();
        let mut byte = [0u8; 1];
        assert!(
            tokio::time::timeout(Duration::from_millis(20), server.read(&mut byte))
                .await
                .is_err()
        );
        drop(prepared);
        assert_eq!(server.read(&mut byte).await.unwrap(), 0);
        drop(listener);
        assert!(writer.prepare_done().await.is_err());
    }
}
