use super::{As200Config, As200Error, WordOrder, decode, map, model::PlcResult, reader::*};
use std::time::Duration;

fn config(order: WordOrder) -> As200Config {
    As200Config {
        control_start: 25000,
        record_size: 240,
        buffer_count: 2,
        address: "127.0.0.1:0".into(),
        unit_id: 1,
        modbus_address_offset: 0,
        word_order: order,
        timeout: Duration::from_secs(1),
    }
}
fn put32(words: &mut [u16], offset: usize, bits: u32, order: WordOrder) {
    let pair = match order {
        WordOrder::HighWordFirst => [(bits >> 16) as u16, bits as u16],
        WordOrder::LowWordFirst => [bits as u16, (bits >> 16) as u16],
    };
    words[offset..offset + 2].copy_from_slice(&pair);
}
fn control(total: i32, order: WordOrder) -> Vec<u16> {
    let mut raw = vec![0; 20];
    raw[0] = 1;
    raw[1] = 240;
    raw[2] = 2;
    raw[3] = (total % 2) as u16;
    raw[6] = (1 - total % 2) as u16;
    raw[7] = 32767;
    raw[11] = u16::from(total > 0);
    put32(&mut raw, 4, total as u32, order);
    raw
}
fn record(seq: i32, order: WordOrder) -> Vec<u16> {
    let mut raw = vec![0; 240];
    raw[0] = 1;
    raw[1] = 0x8001;
    put32(&mut raw, 2, 123456, order);
    raw[4..7].copy_from_slice(&[0x4241, 0x4443, 0x0045]); // ABCDE, byte thấp trước.
    raw[14..20].copy_from_slice(&[9, 8, 7, 6, 5, 4]);
    raw[20] = 1;
    raw[21] = 123;
    put32(&mut raw, 22, seq as u32, order);
    for (header, points, load) in [(24, 32, 100.25_f32), (120, 128, 200.5_f32)] {
        raw[header] = 1;
        raw[header + 1] = 4;
        put32(&mut raw, header + 4, load.to_bits(), order);
        for index in 0..4 {
            let base = points + index * 22;
            raw[base] = 1;
            raw[base + 1] = 4;
            raw[base + 2] = 4;
            put32(
                &mut raw,
                base + 4,
                (10.5_f32 + index as f32).to_bits(),
                order,
            );
            for attempt in 0..4 {
                put32(
                    &mut raw,
                    base + 6 + attempt * 4,
                    (1.25_f32 + attempt as f32).to_bits(),
                    order,
                );
                put32(
                    &mut raw,
                    base + 8 + attempt * 4,
                    (-0.5_f32 - attempt as f32).to_bits(),
                    order,
                );
            }
        }
    }
    raw[216..240].fill(0xbeef);
    raw
}

#[test]
fn scalar_decoding_both_word_orders_and_signed_boundaries() {
    assert_eq!(decode::int16(&[0xffff], 0).unwrap(), -1);
    assert_eq!(decode::int16(&[0x8000], 0).unwrap(), i16::MIN);
    assert_eq!(decode::uint16(&[0xffff], 0).unwrap(), u16::MAX);
    assert_eq!(
        decode::dint32(&[0xffff, 0xfffe], 0, WordOrder::HighWordFirst).unwrap(),
        -2
    );
    assert_eq!(
        decode::dint32(&[0xfffe, 0xffff], 0, WordOrder::LowWordFirst).unwrap(),
        -2
    );
    assert_eq!(
        decode::dint32(&[0x8000, 0], 0, WordOrder::HighWordFirst).unwrap(),
        i32::MIN
    );
    assert_eq!(
        decode::real32(&[0x3fa0, 0], 0, WordOrder::HighWordFirst).unwrap(),
        1.25
    );
    assert_eq!(
        decode::real32(&[0, 0xc020], 0, WordOrder::LowWordFirst).unwrap(),
        -2.5
    );
    assert!(decode::real32(&[0x7f80, 0], 0, WordOrder::HighWordFirst).is_err());
    assert!(decode::real32(&[0x7fc0, 0], 0, WordOrder::HighWordFirst).is_err());
    assert!(decode::dint32(&[1], 0, WordOrder::HighWordFirst).is_err());
    assert!(decode::dint32(&[], usize::MAX, WordOrder::HighWordFirst).is_err());
}
#[test]
fn ascii_uses_low_byte_first_independent_of_numeric_word_order() {
    let mut words = [0; 10];
    words[..3].copy_from_slice(&[0x4241, 0x4443, 0x0045]);
    assert_eq!(decode::product_code(&words).unwrap(), "ABCDE");
    assert_eq!(decode::product_code(&[0x4141; 10]).unwrap().len(), 20);
    words[4] = 0x41;
    assert!(decode::product_code(&words).is_err());
    words = [0; 10];
    words[0] = 0x80;
    assert!(decode::product_code(&words).is_err());
    assert!(decode::product_code(&[0; 9]).is_err());
}
#[test]
fn offsets_cover_both_slots_and_all_points() {
    assert_eq!(map::record_base(0), Some(25020));
    assert_eq!(map::record_base(1), Some(25260));
    assert_eq!(map::record_base(2), None);
    for (number, butt, tip) in [(1, 32, 128), (2, 54, 150), (3, 76, 172), (4, 98, 194)] {
        assert_eq!(map::point_offset(32, number), Some(butt));
        assert_eq!(map::point_offset(128, number), Some(tip));
    }
    assert_eq!(map::point_offset(32, 0), None);
    assert_eq!(map::point_offset(usize::MAX, 4), None);
    assert_eq!(map::modbus_address(25000, -1), Some(24999));
    assert_eq!(map::modbus_address(25000, -25001), None);
    assert_eq!(map::modbus_address(25000, i32::MAX), None);
}
#[test]
fn full_record_preserves_all_fields_without_legacy_conversion() {
    for order in [WordOrder::HighWordFirst, WordOrder::LowWordFirst] {
        let mut raw = record(100000, order);
        // Decoder giữ nguyên cả giá trị sai nghiệp vụ; validator riêng sẽ từ chối.
        for base in [32, 54, 76, 98, 128, 150, 172, 194] {
            raw[base + 2] = 3;
        }
        let decoded = decode::record(&raw, 1, order).unwrap();
        assert!(decoded.valid_unacknowledged);
        assert_eq!(decoded.status, 0x8001);
        assert_eq!(decoded.record_seq, 100000);
        assert_eq!(decoded.stick_no, 123456);
        assert_eq!(decoded.product_code, "ABCDE");
        assert_eq!(decoded.unknown_header_14_19, [9, 8, 7, 6, 5, 4]);
        assert_eq!(decoded.butt.load, 100.25);
        assert_eq!(decoded.tip.load, 200.5);
        for station in [&decoded.butt, &decoded.tip] {
            assert_eq!(station.point_count, 4);
            for (index, point) in station.points.iter().enumerate() {
                assert_eq!(point.position, 10.5 + index as f32);
                assert_eq!(point.attempt_count, 4);
                assert_eq!(point.decide_attempt, 3);
                for (attempt, value) in point.attempts.iter().enumerate() {
                    assert_eq!(value.angle_degrees, attempt as u16 * 90);
                    assert_eq!(value.push_dist, 1.25 + attempt as f32);
                    assert_eq!(value.bend, -0.5 - attempt as f32);
                }
            }
        }
        assert_eq!(decoded.reserve_end, vec![0xbeef; 24]);
        assert_eq!(decoded.raw_registers, raw);
    }
}
#[test]
fn rejects_bad_metadata_codes_and_attempt_ranges() {
    let order = WordOrder::LowWordFirst;
    for offset in [0, 1, 2, 3, 6, 11, 12] {
        let mut raw = control(1, order);
        raw[offset] = 999;
        assert!(decode::control(&raw, order).is_err());
    }
    for (offset, value) in [
        (0, 2),
        (20, 3),
        (24, 3),
        (32, 3),
        (33, 5),
        (34, 5),
        (122, 5),
    ] {
        let mut raw = record(1, order);
        raw[offset] = value;
        assert!(decode::record(&raw, 0, order).is_err(), "offset {offset}");
    }
    assert!(decode::record(&[0; 239], 0, order).is_err());
    assert!(decode::record(&record(1, order), 2, order).is_err());
    let mut raw = record(1, order);
    raw[20] = 0;
    assert_eq!(
        decode::record(&raw, 0, order).unwrap().overall_result,
        PlcResult::Unmeasured
    );
    assert!(!decode::record(&raw, 0, order).unwrap().complete());
    raw[1] = 0x8000;
    assert!(!decode::record(&raw, 0, order).unwrap().valid_unacknowledged);
}

#[derive(Clone)]
struct MemoryReader {
    words: Vec<u16>,
    calls: Vec<(u16, u16)>,
    offset: i32,
    mutate_on_call: Option<usize>,
    mutation_offset: usize,
    short_on_call: Option<usize>,
    fail_on_call: Option<usize>,
}
impl MemoryReader {
    fn new(order: WordOrder, total: i32) -> Self {
        let mut words = control(total, order);
        words.extend(record(total - 1, order));
        words.extend(record(total, order));
        Self {
            words,
            calls: Vec::new(),
            offset: 0,
            mutate_on_call: None,
            mutation_offset: 100,
            short_on_call: None,
            fail_on_call: None,
        }
    }
}
impl HoldingRegisterReader for MemoryReader {
    async fn read_holding_registers(
        &mut self,
        address: u16,
        count: u16,
    ) -> Result<Vec<u16>, As200Error> {
        self.calls.push((address, count));
        let call = self.calls.len();
        if self.fail_on_call == Some(call) {
            return Err(As200Error::Transport("mất kết nối giả lập".into()));
        }
        let base = (i32::from(address) - self.offset - 25000) as usize;
        if self.mutate_on_call == Some(call) {
            self.words[self.mutation_offset] ^= 1;
        }
        let mut words = self.words[base..base + count as usize].to_vec();
        if self.short_on_call == Some(call) {
            words.pop();
        }
        Ok(words)
    }
}
#[tokio::test]
async fn published_record_reads_only_first_region_and_accepts_ready_held_high() {
    for order in [WordOrder::LowWordFirst, WordOrder::HighWordFirst] {
        let mut reader = As200Reader::with_transport(config(order), MemoryReader::new(order, 12));
        let first = reader.poll_published_record().await.unwrap();
        assert!(first.diagnostics.is_empty());
        assert_eq!(first.control.buffer_count, 2);
        assert_eq!(first.control.total_count, 12);
        assert_eq!(first.buffer.unwrap().record.unwrap().record_seq, 11);
        assert_eq!(
            reader.transport.calls,
            vec![
                (25000, 20),
                (25020, 100),
                (25120, 100),
                (25220, 40),
                (25000, 20),
                (25020, 100),
                (25120, 100),
                (25220, 40),
                (25000, 20),
            ]
        );
        // PLC đưa record kế tiếp ra cùng vùng, không đổi TotalCount/DataReady.
        reader.transport.words[20..260].copy_from_slice(&record(12, order));
        let next = reader.poll_published_record().await.unwrap();
        assert!(next.diagnostics.is_empty());
        assert_eq!(next.buffer.unwrap().record.unwrap().record_seq, 12);
    }
}

#[tokio::test]
async fn published_record_not_ready_reads_only_control_then_observes_ready_level() {
    let order = WordOrder::LowWordFirst;
    let mut memory = MemoryReader::new(order, 2);
    memory.words[11] = 0;
    let mut reader = As200Reader::with_transport(config(order), memory);
    let snapshot = reader.poll_published_record().await.unwrap();
    assert!(snapshot.buffer.is_none());
    assert!(snapshot.diagnostics.is_empty());
    assert_eq!(reader.transport.calls, vec![(25000, 20), (25000, 20)]);
    reader.transport.words[11] = 1;
    let snapshot = reader.poll_published_record().await.unwrap();
    assert!(snapshot.buffer.is_some());
    assert!(snapshot.diagnostics.is_empty());
}

#[tokio::test]
async fn published_record_rejects_changing_raw_control_and_short_reads() {
    let order = WordOrder::LowWordFirst;
    // Call 5: control giữa hai lần đọc; call 6: bắt đầu đọc lại raw.
    for (call, offset) in [(6, 100), (5, 11), (5, 8), (5, 12)] {
        let mut memory = MemoryReader::new(order, 2);
        memory.mutate_on_call = Some(call);
        memory.mutation_offset = offset;
        let mut reader = As200Reader::with_transport(config(order), memory);
        assert_eq!(
            reader.poll_published_record().await.unwrap_err(),
            As200Error::Unstable
        );
    }
    let mut memory = MemoryReader::new(order, 2);
    memory.short_on_call = Some(2);
    let mut reader = As200Reader::with_transport(config(order), memory);
    assert!(reader.poll_published_record().await.is_err());
}

#[tokio::test]
async fn published_record_preserves_raw_without_enforcing_projection_validation() {
    let order = WordOrder::LowWordFirst;
    // Status thiếu valid, workflow sai, metadata vượt TotalCount, REAL NaN.
    for (offset, value) in [(21, 0), (45, 3), (42, 3), (49, 0x7fc0)] {
        let mut memory = MemoryReader::new(order, 2);
        memory.words[offset] = value;
        let expected_raw = memory.words[20..260].to_vec();
        let mut reader = As200Reader::with_transport(config(order), memory);
        let snapshot = reader.poll_published_record().await.unwrap();
        let buffer = snapshot.buffer.unwrap();
        assert_eq!(buffer.raw_registers, expected_raw);
        // Workflow and identity are validated by projection/acquisition, not FC03.
        if offset == 21 || offset == 49 {
            assert!(!snapshot.diagnostics.is_empty());
        }
    }
}

// ---- Transport tokio-modbus qua TCP loopback thật (không cần PLC) ----

/// Fake Modbus TCP server: nhận tối đa `connections` kết nối; với mỗi request FC03
/// (12 byte), `handler(kết nối, request)` trả response hoặc `None` để đóng socket.
fn fake_fc03_server<F>(
    listener: tokio::net::TcpListener,
    connections: usize,
    mut handler: F,
) -> tokio::task::JoinHandle<Vec<[u8; 12]>>
where
    F: FnMut(usize, &[u8; 12]) -> Option<Vec<u8>> + Send + 'static,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    tokio::spawn(async move {
        let mut seen = Vec::new();
        for connection in 0..connections {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 12];
            while stream.read_exact(&mut request).await.is_ok() {
                seen.push(request);
                match handler(connection, &request) {
                    Some(response) => stream.write_all(&response).await.unwrap(),
                    None => break,
                }
            }
        }
        seen
    })
}
/// Response FC03 hợp lệ, echo transaction/unit của request.
fn fc03_response(request: &[u8; 12], words: &[u16]) -> Vec<u8> {
    let mut response = request[..7].to_vec();
    response[4..6].copy_from_slice(&((words.len() * 2 + 3) as u16).to_be_bytes());
    response.extend([3, (words.len() * 2) as u8]);
    for word in words {
        response.extend(word.to_be_bytes());
    }
    response
}
async fn loopback(cfg: &mut As200Config) -> tokio::net::TcpListener {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    cfg.address = listener.local_addr().unwrap().to_string();
    listener
}

#[tokio::test]
async fn tcp_fc03_exception_response_is_protocol_error_and_reconnects() {
    let mut cfg = config(WordOrder::LowWordFirst);
    let listener = loopback(&mut cfg).await;
    let server = fake_fc03_server(listener, 2, |connection, request| {
        Some(if connection == 0 {
            // 0x83 = exception FC03, code 2 = Illegal Data Address.
            vec![request[0], request[1], 0, 0, 0, 3, 1, 0x83, 2]
        } else {
            fc03_response(request, &[7, 8])
        })
    });
    let mut transport = ModbusTcpReader::new(cfg);
    match transport.read_holding_registers(25000, 2).await {
        Err(As200Error::Protocol(message)) => assert!(message.contains("exception"), "{message}"),
        other => panic!("cần lỗi exception, nhận {other:?}"),
    }
    // Lỗi phải hủy kết nối cũ; lần sau mở kết nối mới (connection 1) và đọc được.
    assert_eq!(
        transport.read_holding_registers(25000, 2).await.unwrap(),
        vec![7, 8]
    );
    drop(transport);
    server.await.unwrap();
}

#[tokio::test]
async fn tcp_fc03_wrong_unit_byte_count_function_or_short_data_is_rejected() {
    for fault in ["unit", "byte-count", "function", "short", "transaction"] {
        let mut cfg = config(WordOrder::HighWordFirst);
        let listener = loopback(&mut cfg).await;
        let server = fake_fc03_server(listener, 1, move |_, request| {
            let mut response = fc03_response(request, &[1, 2]);
            match fault {
                "unit" => response[6] = 9,
                "byte-count" => response[8] = 2,
                "function" => response[7] = 4,
                "transaction" => response[1] = response[1].wrapping_add(1),
                _ => response = fc03_response(request, &[1]),
            }
            Some(response)
        });
        let mut transport = ModbusTcpReader::new(cfg);
        let result = transport.read_holding_registers(25000, 2).await;
        assert!(
            matches!(result, Err(As200Error::Protocol(_))),
            "{fault}: {result:?}"
        );
        drop(transport);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn tcp_reuses_connection_and_increments_transaction_id() {
    let mut cfg = config(WordOrder::LowWordFirst);
    let listener = loopback(&mut cfg).await;
    let server = fake_fc03_server(listener, 1, |_, request| {
        let address = u16::from_be_bytes([request[8], request[9]]);
        Some(fc03_response(request, &[address, 0xffff]))
    });
    let mut transport = ModbusTcpReader::new(cfg);
    for address in [25000, 25020, 25120] {
        assert_eq!(
            transport.read_holding_registers(address, 2).await.unwrap(),
            vec![address, 0xffff]
        );
    }
    drop(transport);
    // Server chỉ accept 1 kết nối: cả 3 request đi chung socket.
    let seen = server.await.unwrap();
    assert_eq!(seen.len(), 3);
    let ids: std::collections::HashSet<_> = seen.iter().map(|r| [r[0], r[1]]).collect();
    assert_eq!(ids.len(), 3, "transaction id phải khác nhau");
    for request in &seen {
        assert_eq!(&request[2..8], &[0, 0, 0, 6, 1, 3]);
    }
}

#[tokio::test]
async fn tcp_connect_refused_or_bad_address_is_error_without_panic() {
    let mut cfg = config(WordOrder::LowWordFirst);
    drop(loopback(&mut cfg).await); // Cổng vừa đóng: không có server.
    cfg.timeout = Duration::from_millis(300);
    let mut transport = ModbusTcpReader::new(cfg.clone());
    let result = transport.read_holding_registers(25000, 2).await;
    assert!(
        matches!(result, Err(As200Error::Transport(_) | As200Error::Timeout)),
        "{result:?}"
    );
    cfg.address = "plc-khong-phai-ip:502".into();
    let mut transport = ModbusTcpReader::new(cfg);
    assert!(matches!(
        transport.read_holding_registers(25000, 2).await,
        Err(As200Error::Transport(_))
    ));
}

#[tokio::test]
async fn published_snapshot_over_tcp_never_reads_second_region() {
    for order in [WordOrder::LowWordFirst, WordOrder::HighWordFirst] {
        let mut cfg = config(order);
        cfg.modbus_address_offset = -1;
        let listener = loopback(&mut cfg).await;
        let mut memory = control(2, order);
        memory.extend(record(1, order));
        let server = fake_fc03_server(listener, 1, move |_, request| {
            let address = usize::from(u16::from_be_bytes([request[8], request[9]]));
            let count = usize::from(u16::from_be_bytes([request[10], request[11]]));
            assert!(count <= map::FC03_LIMIT);
            let base = address + 1 - 25000;
            assert!(base + count <= 260, "No Buffer 2 reads");
            Some(fc03_response(request, &memory[base..base + count]))
        });
        let mut reader = As200Reader::new(cfg);
        let snapshot = reader.poll_published_record().await.unwrap();
        assert!(snapshot.diagnostics.is_empty());
        assert_eq!(snapshot.control.total_count, 2);
        assert_eq!(snapshot.buffer.unwrap().record.unwrap().record_seq, 1);
        drop(reader);
        assert!(server.await.unwrap().iter().all(|r| r[7] == 3));
    }
}

#[tokio::test]
async fn relocated_profile_reads_only_published_words_with_order_offset_and_chunk_limits() {
    for order in [WordOrder::LowWordFirst, WordOrder::HighWordFirst] {
        for offset in [-1, 0, 7] {
            let mut cfg = config(order);
            cfg.control_start = 26000;
            cfg.modbus_address_offset = offset;
            let mut memory = MemoryReader::new(order, 12);
            memory.offset = 1000 + offset;
            let mut reader = As200Reader::with_transport(cfg, memory);
            let snapshot = reader.poll_published_record().await.unwrap();
            assert_eq!(snapshot.buffer.unwrap().record.unwrap().record_seq, 11);
            let first = (26000 + offset) as u16;
            assert_eq!(
                reader.transport.calls,
                vec![
                    (first, 20),
                    (first + 20, 100),
                    (first + 120, 100),
                    (first + 220, 40),
                    (first, 20),
                    (first + 20, 100),
                    (first + 120, 100),
                    (first + 220, 40),
                    (first, 20)
                ]
            );
            assert!(
                reader
                    .transport
                    .calls
                    .iter()
                    .all(|(address, count)| *count <= 100
                        && u32::from(*address) + u32::from(*count) <= u32::from(first) + 260)
            );
        }
    }
}
#[tokio::test]
async fn unsupported_profile_is_rejected_before_reading_registers() {
    for (count, size) in [(3, 240), (2, 256), (2, 300)] {
        let mut cfg = config(WordOrder::LowWordFirst);
        cfg.buffer_count = count;
        cfg.record_size = size;
        let mut reader =
            As200Reader::with_transport(cfg, MemoryReader::new(WordOrder::LowWordFirst, 2));
        assert!(reader.poll_published_record().await.is_err());
        assert!(reader.transport.calls.is_empty());
    }
}

#[tokio::test]
async fn arbitrary_raw_total_zero_then_one_keeps_all_words_and_plc_identity() {
    let order = WordOrder::LowWordFirst;
    for single in [false, true] {
        let mut memory = MemoryReader::new(order, 0);
        memory.words[11] = 1;
        memory.words[20..260].fill(0);
        if single {
            memory.words[200] = 12;
        }
        let expected = memory.words[20..260].to_vec();
        let mut reader = As200Reader::with_transport(config(order), memory);
        let zero = reader.poll_published_record().await.unwrap();
        assert_eq!(zero.control.total_count, 0);
        let raw = zero.buffer.unwrap();
        assert_eq!(raw.raw_registers, expected);
        assert!(raw.record.is_none());
        put32(&mut reader.transport.words, 4, 1, order);
        let one = reader.poll_published_record().await.unwrap();
        assert_eq!(one.control.total_count, 1);
        assert_eq!(one.buffer.unwrap().raw_registers, expected);
    }
}
#[tokio::test]
async fn repeated_identical_payloads_have_distinct_control_counters_for_one_two_five_ten_reads() {
    let order = WordOrder::LowWordFirst;
    let mut memory = MemoryReader::new(order, 1);
    memory.words[20..260].fill(0);
    let mut reader = As200Reader::with_transport(config(order), memory);
    let mut seen = Vec::new();
    for total in 1..=10 {
        put32(&mut reader.transport.words, 4, total as u32, order);
        let snap = reader.poll_published_record().await.unwrap();
        seen.push(snap.control.total_count);
        assert_eq!(snap.buffer.unwrap().raw_registers, vec![0; 240]);
        if [1, 2, 5, 10].contains(&total) {
            assert_eq!(seen, (1..=total).collect::<Vec<_>>());
        }
    }
}
#[tokio::test]
async fn read_failure_recovers_and_counter_change_mid_snapshot_is_unstable() {
    let order = WordOrder::LowWordFirst;
    let mut memory = MemoryReader::new(order, 1);
    memory.fail_on_call = Some(2);
    let mut reader = As200Reader::with_transport(config(order), memory);
    assert!(matches!(
        reader.poll_published_record().await,
        Err(As200Error::Transport(_))
    ));
    reader.transport.fail_on_call = None;
    assert_eq!(
        reader
            .poll_published_record()
            .await
            .unwrap()
            .control
            .total_count,
        1
    );
    let mut memory = MemoryReader::new(order, 1);
    memory.mutate_on_call = Some(5);
    memory.mutation_offset = 4;
    let mut reader = As200Reader::with_transport(config(order), memory);
    assert_eq!(
        reader.poll_published_record().await.unwrap_err(),
        As200Error::Unstable
    );
}
#[tokio::test]
async fn fc03_response_timeout_is_not_a_measurement_or_successful_snapshot() {
    let mut cfg = config(WordOrder::LowWordFirst);
    cfg.timeout = Duration::from_millis(40);
    let listener = loopback(&mut cfg).await;
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 12];
        stream.read_exact(&mut request).await.unwrap();
        let _ = stop_rx.await;
    });
    let mut reader = As200Reader::new(cfg);
    assert_eq!(
        reader.poll_published_record().await.unwrap_err(),
        As200Error::Timeout
    );
    let _ = stop_tx.send(());
    server.await.unwrap();
}
#[tokio::test]
async fn count_response_lost_returns_error_and_transport_does_not_replay_request() {
    use tokio::io::AsyncReadExt;
    let mut cfg = config(WordOrder::LowWordFirst);
    let listener = loopback(&mut cfg).await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 17];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(&request[7..], &[16, 97, 176, 0, 2, 4, 0, 0, 0, 1]); // 65536 LOW: [0,1]
        drop(stream);
        assert!(
            tokio::time::timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err()
        );
    });
    let writer = super::ack::AckWriter::new(cfg, "modbus", true, Some("as200"), true).unwrap();
    let prepared = writer.prepare_count().await.unwrap();
    assert!(writer.count_prepared(65536, prepared).await.is_err());
    server.await.unwrap();
}
