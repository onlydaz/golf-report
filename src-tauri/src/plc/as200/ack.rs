//! Whitelisted AS200 Count/Done transport; acquisition owns durable ACK evidence.
use super::As200Config;
use super::{WordOrder, map};
use tokio::net::TcpStream;
use tokio_modbus::{
    Request, Response, Slave,
    client::{Client, tcp},
};

/// Lệnh ghi duy nhất được phép; helper `send` là private nên không có API ghi tùy ý.
#[derive(Debug, Clone, Copy)]
enum Write {
    /// FC06: địa chỉ, giá trị.
    Single(u16, u16),
    /// FC16: địa chỉ, đúng 2 WORD (DINT32).
    Multiple(u16, [u16; 2]),
}

/// Chỉ constructor này cấp quyền ghi, dựa trên mode/protocol và opt-in rõ ràng.
pub struct AckWriter {
    cfg: As200Config,
}
impl AckWriter {
    pub fn new(
        cfg: As200Config,
        mode: &str,
        enabled: bool,
        protocol: Option<&str>,
        allow_plc_connection: bool,
    ) -> Result<Self, String> {
        let end = usize::from(cfg.control_start)
            .checked_add(map::CONTROL_SIZE + map::BUFFER_COUNT * map::RECORD_SIZE)
            .filter(|v| *v <= 65536)
            .ok_or("ACK layout address overflow")?;
        if cfg.record_size != map::RECORD_SIZE
            || cfg.buffer_count != map::BUFFER_COUNT
            || map::modbus_address(cfg.control_start, cfg.modbus_address_offset).is_none()
            || map::modbus_address((end - 1) as u16, cfg.modbus_address_offset).is_none()
        {
            return Err("ACK layout chưa tương thích hoặc vùng địa chỉ không hợp lệ".into());
        }
        let address: std::net::SocketAddr = cfg
            .address
            .parse()
            .map_err(|_| "ACK requires numeric IP:port")?;
        let permitted =
            matches!(mode, "plc" | "modbus") && allow_plc_connection && protocol == Some("as200");
        if !enabled || !permitted || address.port() == 0 {
            return Err("ACK write capability disabled".into());
        }
        Ok(Self { cfg })
    }
    // Không có API địa chỉ tùy ý: FC16 chỉ D25008..09, FC06 chỉ D25012=1.
    #[cfg(test)]
    async fn count(&self, count: i32) -> Result<(), String> {
        if count <= 0 {
            return Err("ACK invalid count".into());
        }
        self.count_prepared(count, self.prepare_count().await?)
            .await
    }
    /// Connect without sending a Modbus request, before the durable Count marker.
    pub async fn prepare_count(&self) -> Result<TcpStream, String> {
        super::connect_tcp(&self.cfg.address, self.cfg.local_bind_ip, self.cfg.timeout)
            .await
            .map_err(|e| format!("ACK_FC16_CONNECT: Chưa gọi FC16 Count; {e}"))
    }
    pub async fn count_prepared(&self, count: i32, stream: TcpStream) -> Result<(), String> {
        if count <= 0 {
            return Err("ACK invalid count".into());
        }
        let address = map::modbus_address(
            self.cfg.control_start + map::control::PC_READ_COUNT as u16,
            self.cfg.modbus_address_offset,
        )
        .ok_or("ACK address overflow")?;
        let bits = count as u32;
        let mut words = [(bits >> 16) as u16, bits as u16];
        if matches!(self.cfg.word_order, WordOrder::LowWordFirst) {
            words.reverse();
        }
        send(&self.cfg, Write::Multiple(address, words), stream).await
    }
    /// Mở socket chưa gửi byte nào: lỗi connect không làm intent thành "đã gửi".
    pub async fn prepare_done(&self) -> Result<TcpStream, String> {
        super::connect_tcp(&self.cfg.address, self.cfg.local_bind_ip, self.cfg.timeout)
            .await
            .map_err(|e| format!("ACK_FC06_CONNECT: Chưa gọi FC06 Done; {e}"))
    }
    pub async fn done_prepared(&self, stream: TcpStream) -> Result<(), String> {
        let address = map::modbus_address(
            self.cfg.control_start + map::control::PC_READ_DONE as u16,
            self.cfg.modbus_address_offset,
        )
        .ok_or("ACK address overflow")?;
        send(&self.cfg, Write::Single(address, 1), stream).await
    }
}

/// Gửi một lệnh ghi qua `tokio-modbus` và chỉ coi là thành công khi PLC echo
/// đúng địa chỉ/giá trị (FC06) hoặc địa chỉ/số lượng (FC16). Exception, sai
/// header (transaction/unit), sai function code, mất kết nối hay timeout đều là lỗi.
async fn send(cfg: &As200Config, write: Write, stream: TcpStream) -> Result<(), String> {
    let function = match write {
        Write::Single(..) => "FC06",
        Write::Multiple(..) => "FC16",
    };
    crate::plc::diagnostics::write_outcome(function, "CALL_START");
    // A call/echo is transport evidence, never proof of Ladder/checkpoint completion.
    crate::plc::diagnostics::event!(
        "PLC_ACK_TRANSPORT stage=CALL_START endpoint={} unit_id={} function={function} request={write:?} word_order={:?} timeout_ms={} checkpoint_confirmed=false",
        cfg.address, cfg.unit_id, cfg.word_order, cfg.timeout.as_millis()
    );
    let operation = async {
        let mut ctx = tcp::attach_slave(stream, Slave(cfg.unit_id));
        let (request, expected) = match write {
            Write::Single(address, value) => (
                Request::WriteSingleRegister(address, value),
                Response::WriteSingleRegister(address, value),
            ),
            Write::Multiple(address, words) => (
                Request::WriteMultipleRegisters(address, words.to_vec().into()),
                Response::WriteMultipleRegisters(address, words.len() as u16),
            ),
        };
        let response = ctx
            .call(request)
            .await
            .map_err(|e| {
                crate::plc::diagnostics::write_outcome(function, "RESPONSE_ERROR");
                crate::plc::diagnostics::event!("PLC_ACK_TRANSPORT stage=RESPONSE_ERROR endpoint={} unit_id={} function={function} request={write:?} error={e} write_outcome=unconfirmed", cfg.address, cfg.unit_id);
                format!("ACK_{function}_RESPONSE: ACK invalid response: {e}; kết quả ghi chưa được xác minh, giữ intent và không tự gửi lại.")
            })?
            .map_err(|e| {
                crate::plc::diagnostics::write_outcome(function, "PLC_EXCEPTION");
                crate::plc::diagnostics::event!("PLC_ACK_TRANSPORT stage=PLC_EXCEPTION endpoint={} unit_id={} function={function} request={write:?} exception={e} checkpoint_confirmed=false", cfg.address, cfg.unit_id);
                format!("ACK_{function}_EXCEPTION: ACK refused by PLC exception: {e}; giữ intent để đối chiếu, không tự gửi lại.")
            })?;
        if response != expected {
            crate::plc::diagnostics::write_outcome(function, "ECHO_MISMATCH");
            crate::plc::diagnostics::event!("PLC_ACK_TRANSPORT stage=ECHO_MISMATCH endpoint={} unit_id={} function={function} request={write:?} response={response:?} expected={expected:?} write_outcome=unconfirmed", cfg.address, cfg.unit_id);
            return Err(format!("ACK_{function}_ECHO_MISMATCH: ACK refused/malformed response: {response:?}; chưa xác minh kết quả ghi, không tự gửi lại."));
        }
        crate::plc::diagnostics::write_outcome(function, "ECHO_OK");
        crate::plc::diagnostics::event!("PLC_ACK_TRANSPORT stage=ECHO_OK endpoint={} unit_id={} function={function} request={write:?} response={response:?} readback_required=true checkpoint_confirmed=false", cfg.address, cfg.unit_id);
        Ok(())
    };
    tokio::time::timeout(cfg.timeout, operation)
        .await
        .map_err(|_| {
            crate::plc::diagnostics::write_outcome(function, "TIMEOUT");
            crate::plc::diagnostics::event!("PLC_ACK state=TIMEOUT endpoint={} unit_id={} function={function} request={write:?} write_outcome=unconfirmed", cfg.address, cfg.unit_id);
            format!("ACK_{function}_TIMEOUT: Hết thời gian chờ kết quả ghi; PLC có thể đã nhận lệnh. Giữ intent để đối chiếu, không tự gửi lại.")
        })?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn send_done(writer: &AckWriter) -> Result<(), String> {
        writer.done_prepared(writer.prepare_done().await?).await
    }

    #[tokio::test]
    async fn fc16_word_order_offset_and_fc06_echo_are_checked() {
        for order in [WordOrder::LowWordFirst, WordOrder::HighWordFirst] {
            for mode in ["modbus", "plc"] {
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
                        .write_all(&[request[0], request[1], 0, 0, 0, 6, 1, 16, 97, 175, 0, 2])
                        .await
                        .unwrap();
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut request = [0; 12];
                    stream.read_exact(&mut request).await.unwrap();
                    assert_eq!(&request[7..], &[6, 97, 179, 0, 1]); // D25012 - 1
                    stream.write_all(&request).await.unwrap();
                });
                let cfg = As200Config {
                    local_bind_ip: None,
                    control_start: 25000,
                    record_size: 240,
                    buffer_count: 2,
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
                let writer = AckWriter::new(cfg, mode, true, Some("as200"), true).unwrap();
                writer.count(65538).await.unwrap();
                send_done(&writer).await.unwrap();
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
                        request[1] = request[1].wrapping_add(1);
                        stream.write_all(&request).await.unwrap();
                    }
                    _ => {}
                }
            });
            let cfg = As200Config {
                local_bind_ip: None,
                control_start: 25000,
                record_size: 240,
                buffer_count: 2,
                address,
                unit_id: 1,
                modbus_address_offset: 0,
                word_order: WordOrder::LowWordFirst,
                timeout: Duration::from_millis(50),
            };
            let writer = AckWriter::new(cfg, "modbus", true, Some("as200"), true).unwrap();
            assert!(send_done(&writer).await.is_err());
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn prepared_done_does_not_send_until_explicitly_consumed() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let cfg = As200Config {
            local_bind_ip: None,
            control_start: 25000,
            record_size: 240,
            buffer_count: 2,
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

    /// Fake server một request: đọc `len` byte, trả `reply(request)` rồi đóng socket.
    async fn one_shot(
        len: usize,
        reply: impl FnOnce(Vec<u8>) -> Vec<u8> + Send + 'static,
    ) -> (As200Config, tokio::task::JoinHandle<Vec<u8>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let cfg = As200Config {
            local_bind_ip: None,
            control_start: 25000,
            record_size: 240,
            buffer_count: 2,
            address: listener.local_addr().unwrap().to_string(),
            unit_id: 1,
            modbus_address_offset: -1,
            word_order: WordOrder::LowWordFirst,
            timeout: Duration::from_millis(300),
        };
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0u8; len];
            stream.read_exact(&mut request).await.unwrap();
            stream.write_all(&reply(request.clone())).await.unwrap();
            request
        });
        (cfg, server)
    }
    fn ack_writer(cfg: As200Config) -> AckWriter {
        AckWriter::new(cfg, "plc", true, Some("as200"), true).unwrap()
    }

    #[tokio::test]
    async fn fc06_wrong_echo_value_or_address_is_rejected() {
        // Done (D25012=1): PLC echo sai giá trị hoặc sai địa chỉ → không được coi là đã gửi xong.
        for byte in [11, 9] {
            let (cfg, server) = one_shot(12, move |mut r| {
                r[byte] ^= 1; // 11 = byte thấp giá trị, 9 = byte thấp địa chỉ.
                r
            })
            .await;
            let error = send_done(&ack_writer(cfg)).await.unwrap_err();
            assert!(error.contains("malformed"), "{error}");
            let request = server.await.unwrap();
            assert_eq!(&request[7..], &[6, 97, 179, 0, 1]);
        }
    }

    #[tokio::test]
    async fn fc16_wrong_quantity_address_or_exception_is_rejected() {
        let replies: [fn(Vec<u8>) -> Vec<u8>; 3] = [
            |r| vec![r[0], r[1], 0, 0, 0, 6, 1, 16, 97, 175, 0, 1], // quantity 1 thay vì 2
            |r| vec![r[0], r[1], 0, 0, 0, 6, 1, 16, 97, 176, 0, 2], // sai địa chỉ
            |r| vec![r[0], r[1], 0, 0, 0, 3, 1, 0x90, 4],           // exception FC16
        ];
        for (case, reply) in replies.into_iter().enumerate() {
            let (cfg, server) = one_shot(17, reply).await;
            let error = ack_writer(cfg).count(7).await.unwrap_err();
            if case == 2 {
                assert!(error.contains("exception"), "{error}");
            }
            let request = server.await.unwrap();
            // FC16 D25008-1, 2 WORD, LowWordFirst: 7 = [0x0007, 0x0000].
            assert_eq!(&request[7..], &[16, 97, 175, 0, 2, 4, 0, 7, 0, 0]);
        }
    }

    #[tokio::test]
    async fn wrong_unit_id_or_function_in_ack_response_is_rejected() {
        for (index, value) in [(6, 2u8), (7, 3u8)] {
            let (cfg, server) = one_shot(12, move |mut r| {
                r[index] = value;
                r
            })
            .await;
            assert!(send_done(&ack_writer(cfg)).await.is_err(), "byte {index}");
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn prepared_stream_is_used_for_done_without_new_connection() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let cfg = As200Config {
            local_bind_ip: None,
            control_start: 25000,
            record_size: 240,
            buffer_count: 2,
            address: listener.local_addr().unwrap().to_string(),
            unit_id: 1,
            modbus_address_offset: 0,
            word_order: WordOrder::HighWordFirst,
            timeout: Duration::from_millis(300),
        };
        let writer = ack_writer(cfg);
        let prepared = writer.prepare_done().await.unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 12];
            stream.read_exact(&mut request).await.unwrap();
            assert_eq!(&request[6..], &[1, 6, 97, 180, 0, 1]); // D25012, offset 0.
            stream.write_all(&request).await.unwrap();
            // Không có kết nối thứ hai.
            assert!(
                tokio::time::timeout(Duration::from_millis(50), listener.accept())
                    .await
                    .is_err()
            );
        });
        writer.done_prepared(prepared).await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn relocated_ack_uses_profile_control_base_and_offset() {
        for order in [WordOrder::LowWordFirst, WordOrder::HighWordFirst] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap().to_string();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut frame = [0; 17];
                socket.read_exact(&mut frame).await.unwrap();
                assert_eq!(frame[7], 16);
                assert_eq!(u16::from_be_bytes([frame[8], frame[9]]), 26007);
                let expected = if order == WordOrder::LowWordFirst {
                    [0, 2, 0, 1]
                } else {
                    [0, 1, 0, 2]
                };
                assert_eq!(&frame[13..], &expected);
                let reply = [
                    frame[0], frame[1], 0, 0, 0, 6, 1, 16, frame[8], frame[9], 0, 2,
                ];
                socket.write_all(&reply).await.unwrap();
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut frame = [0; 12];
                socket.read_exact(&mut frame).await.unwrap();
                assert_eq!(frame[7], 6);
                assert_eq!(u16::from_be_bytes([frame[8], frame[9]]), 26011);
                assert_eq!(&frame[10..], &[0, 1]);
                socket.write_all(&frame).await.unwrap();
            });
            let cfg = As200Config {
                local_bind_ip: None,
                control_start: 26000,
                record_size: 240,
                buffer_count: 2,
                address,
                unit_id: 1,
                modbus_address_offset: -1,
                word_order: order,
                timeout: Duration::from_millis(1000),
            };
            let writer = AckWriter::new(cfg, "modbus", true, Some("as200"), true).unwrap();
            writer.count(65538).await.unwrap();
            send_done(&writer).await.unwrap();
            server.await.unwrap();
        }
    }
}
