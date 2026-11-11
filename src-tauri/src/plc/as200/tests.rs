use super::{
    As200Config, As200Error, DisabledAcknowledgement, PlcAcknowledgement, WordOrder, decode, map,
    model::PlcResult, reader::*,
};
use std::time::Duration;

fn config(order: WordOrder) -> As200Config {
    As200Config {
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
async fn chunks_240_words_into_125_and_115_and_applies_address_offset() {
    let order = WordOrder::LowWordFirst;
    let mut memory = MemoryReader::new(order, 2);
    memory.offset = -1;
    let mut cfg = config(order);
    cfg.modbus_address_offset = -1;
    let mut reader = As200Reader::with_transport(cfg, memory);
    let report = reader.poll().await.unwrap();
    assert!(report.new_records.is_empty());
    assert_eq!(report.buffers[0].record.as_ref().unwrap().record_seq, 1);
    assert_eq!(report.buffers[1].record.as_ref().unwrap().record_seq, 2);
    assert_eq!(
        reader.transport.calls,
        vec![
            (24999, 20),
            (25019, 125),
            (25144, 115),
            (25259, 125),
            (25384, 115),
            (24999, 20),
            (25019, 125),
            (25144, 115),
            (25259, 125),
            (25384, 115),
            (24999, 20),
        ]
    );
}
#[tokio::test]
async fn detects_new_records_from_both_buffers_and_deduplicates() {
    let order = WordOrder::HighWordFirst;
    let mut reader = As200Reader::with_transport(config(order), MemoryReader::new(order, 2));
    reader.poll().await.unwrap();
    reader.transport.words = MemoryReader::new(order, 4).words;
    let report = reader.poll().await.unwrap();
    assert_eq!(
        report
            .new_records
            .iter()
            .map(|r| r.record_seq)
            .collect::<Vec<_>>(),
        vec![3, 4]
    );
    assert!(reader.poll().await.unwrap().new_records.is_empty());
}
#[tokio::test]
async fn rejects_torn_records_short_reads_and_does_not_advance_checkpoint() {
    let order = WordOrder::LowWordFirst;
    let mut memory = MemoryReader::new(order, 2);
    memory.mutate_on_call = Some(7);
    let mut reader = As200Reader::with_transport(config(order), memory);
    assert_eq!(reader.poll().await.unwrap_err(), As200Error::Unstable);
    assert!(reader.tracker.total_count.is_none());
    reader.transport.mutate_on_call = None;
    reader.transport.short_on_call = Some(13);
    assert!(reader.poll().await.is_err());
    assert!(reader.tracker.total_count.is_none());
}
#[tokio::test]
async fn reports_overrun_and_requires_explicit_session_confirmation_after_reset() {
    let order = WordOrder::LowWordFirst;
    let mut reader = As200Reader::with_transport(config(order), MemoryReader::new(order, 2));
    reader.poll().await.unwrap();
    reader.transport.words = MemoryReader::new(order, 7).words;
    let report = reader.poll().await.unwrap();
    assert!(report.diagnostics.contains(&Diagnostic::BufferOverrunRisk {
        created_since_last_poll: 5,
        capacity: 2
    }));
    assert!(
        report
            .diagnostics
            .contains(&Diagnostic::UnaccountedRecords {
                created_since_last_poll: 5,
                available_new_records: 2
            })
    );
    assert_eq!(report.new_records.len(), 2); // Không tự dựng ba bản ghi đã mất.
    reader.transport.words = MemoryReader::new(order, 2).words;
    assert!(
        reader
            .poll()
            .await
            .unwrap()
            .diagnostics
            .iter()
            .any(|d| matches!(
                d,
                Diagnostic::TrackerRejected {
                    session_unconfirmed: true,
                    ..
                }
            ))
    );
    reader.transport.words = MemoryReader::new(order, 8).words;
    assert!(
        reader
            .poll()
            .await
            .unwrap()
            .diagnostics
            .iter()
            .any(|d| matches!(
                d,
                Diagnostic::TrackerRejected {
                    session_unconfirmed: true,
                    ..
                }
            ))
    );
    reader.tracker.reset_after_confirmed_session_change();
    assert!(reader.poll().await.unwrap().new_records.is_empty());
}
#[tokio::test]
async fn handles_empty_slots_incomplete_records_and_reconnection() {
    let order = WordOrder::LowWordFirst;
    let mut memory = MemoryReader::new(order, 2);
    memory.words[20..260].fill(0);
    memory.fail_on_call = Some(1);
    let mut reader = As200Reader::with_transport(config(order), memory);
    assert!(reader.poll().await.is_err());
    let report = reader.poll().await.unwrap();
    assert!(report.buffers[0].record.is_none());
    reader.transport.words = MemoryReader::new(order, 3).words;
    reader.transport.words[260 + 20] = 0;
    let report = reader.poll().await.unwrap();
    assert!(report.new_records.is_empty());
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| matches!(d, Diagnostic::IncompleteRecord { record_seq: 3, .. }))
    );
    assert_eq!(reader.tracker.total_count, Some(2));
}
#[test]
fn ack_is_explicitly_disabled() {
    assert_eq!(
        DisabledAcknowledgement.acknowledge(10),
        Err(As200Error::AckDisabled)
    );
}

#[test]
fn excel_workflow_checks_results_points_attempts_and_unused_fields() {
    let order = WordOrder::LowWordFirst;
    let mut raw = record(1, order);
    for base in [32, 54, 76, 98, 128, 150, 172, 194] {
        raw[base + 2] = 4;
    }
    assert!(
        decode::record(&raw, 0, order)
            .unwrap()
            .validate_workflow()
            .is_ok()
    );
    for (header, points) in [(24, 32), (120, 128)] {
        let mut ng = raw.clone();
        ng[20] = 2;
        ng[header] = 2;
        ng[header + 1] = 2;
        ng[header + 2] = 2;
        ng[points + 22] = 2;
        ng[points + 44..points + 88].fill(0);
        assert!(
            decode::record(&ng, 0, order)
                .unwrap()
                .validate_workflow()
                .is_ok()
        );
        // Enum/range vẫn đúng nhưng nghiệp vụ NG trước lượt thứ tư là sai.
        let mut early = ng.clone();
        early[points + 23] = 2;
        early[points + 24] = 2;
        assert!(
            decode::record(&early, 0, order)
                .unwrap()
                .validate_workflow()
                .is_err()
        );
        for (offset, value) in [
            (20, 1),
            (header + 1, 3),
            (header + 2, 3),
            (points, 0),
            (points + 24, 3),
            (points + 44, 1),
            (points + 50, 1),
        ] {
            let mut invalid = ng.clone();
            invalid[offset] = value;
            assert!(
                decode::record(&invalid, 0, order)
                    .map(|r| r.validate_workflow().is_err())
                    .unwrap_or(true),
                "offset {offset}"
            );
        }
    }
    let mut first_ok = raw;
    first_ok[33] = 1;
    first_ok[34] = 1;
    first_ok[42..54].fill(0);
    assert!(
        decode::record(&first_ok, 0, order)
            .unwrap()
            .validate_workflow()
            .is_ok()
    );
    first_ok[42] = 1;
    assert!(
        decode::record(&first_ok, 0, order)
            .unwrap()
            .validate_workflow()
            .is_err()
    );
}

#[tokio::test]
async fn stable_decode_failure_preserves_raw_and_the_other_slot_without_checkpoint_advance() {
    let order = WordOrder::LowWordFirst;
    for bad_offset in [20, 36] {
        let mut memory = MemoryReader::new(order, 2);
        if bad_offset == 20 {
            memory.words[20 + bad_offset] = 9;
        } else {
            put32(
                &mut memory.words,
                20 + bad_offset,
                f32::NAN.to_bits(),
                order,
            );
        }
        let original = memory.words[20..260].to_vec();
        let mut reader = As200Reader::with_transport(config(order), memory);
        let snapshot = reader.poll().await.unwrap();
        assert_eq!(snapshot.buffers[0].raw_registers, original);
        assert!(snapshot.buffers[0].record.is_none());
        assert_eq!(snapshot.buffers[1].record.as_ref().unwrap().record_seq, 2);
        assert!(matches!(
            &snapshot.diagnostics[0],
            Diagnostic::InvalidRecord {
                buffer_index: 0,
                ..
            }
        ));
        assert!(reader.tracker.total_count.is_none());
        assert!(reader.poll().await.unwrap().new_records.is_empty());
    }
}

#[tokio::test]
async fn business_error_is_returned_before_tracker_can_hide_its_raw() {
    let order = WordOrder::LowWordFirst;
    let mut reader = As200Reader::with_transport(config(order), MemoryReader::new(order, 2));
    reader.poll().await.unwrap();
    // Cùng seq đổi raw: validation nghiệp vụ phải giữ được raw trước tracker.
    reader.transport.words[20 + 20] = 2; // Overall NG nhưng hai trạm OK.
    let raw = reader.transport.words[20..260].to_vec();
    let snapshot = reader.poll().await.unwrap();
    assert_eq!(snapshot.buffers[0].raw_registers, raw);
    assert!(matches!(
        &snapshot.diagnostics[0],
        Diagnostic::InvalidRecord {
            buffer_index: 0,
            ..
        }
    ));
    assert_eq!(snapshot.buffers[1].record.as_ref().unwrap().record_seq, 2);
    assert_eq!(reader.tracker.total_count, Some(2));
}

#[tokio::test]
async fn control_changes_invalidate_snapshot_but_heartbeat_changes_do_not() {
    let order = WordOrder::LowWordFirst;
    let mut memory = MemoryReader::new(order, 2);
    memory.mutate_on_call = Some(6);
    memory.mutation_offset = 4;
    let mut reader = As200Reader::with_transport(config(order), memory);
    assert_eq!(reader.poll().await.unwrap_err(), As200Error::Unstable);
    assert!(reader.tracker.total_count.is_none());
    let mut memory = MemoryReader::new(order, 2);
    memory.mutate_on_call = Some(6);
    memory.mutation_offset = 7;
    let mut reader = As200Reader::with_transport(config(order), memory);
    assert!(reader.poll().await.is_ok());
}

#[tokio::test]
async fn rejects_duplicate_sequences_and_sequence_changes_without_count_changes() {
    let order = WordOrder::LowWordFirst;
    let mut memory = MemoryReader::new(order, 2);
    put32(&mut memory.words, 20 + 22, 2, order);
    let mut reader = As200Reader::with_transport(config(order), memory);
    assert!(
        reader
            .poll()
            .await
            .unwrap()
            .diagnostics
            .iter()
            .any(|d| matches!(d, Diagnostic::TrackerRejected { .. }))
    );
    reader.transport.words = MemoryReader::new(order, 2).words;
    reader.poll().await.unwrap();
    reader.transport.words[260..500].copy_from_slice(&record(3, order));
    assert!(!reader.poll().await.unwrap().diagnostics.is_empty());
    assert_eq!(reader.tracker.total_count, Some(2));
}

#[tokio::test]
async fn tcp_transport_uses_fc03_validates_mbap_and_reconnects_after_bad_response() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 12];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(&request[2..], &[0, 0, 0, 6, 1, 3, 0x61, 0xa8, 0, 2]); // 25000, 2 WORD.
        let response = [
            request[0], request[1], 0, 0, 0, 7, 1, 3, 4, 0x12, 0x34, 0xab, 0xcd,
        ];
        stream.write_all(&response).await.unwrap();
        stream.read_exact(&mut request).await.unwrap();
        let mut wrong = response;
        wrong[1] = request[1].wrapping_add(1); // Transaction không khớp.
        stream.write_all(&wrong).await.unwrap();
        drop(stream);
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(request[7], 3);
        let response = [request[0], request[1], 0, 0, 0, 7, 1, 3, 4, 0, 1, 0, 2];
        stream.write_all(&response).await.unwrap();
    });
    let mut cfg = config(WordOrder::LowWordFirst);
    cfg.address = address;
    let mut transport = ModbusTcpReader::new(cfg);
    assert_eq!(
        transport.read_holding_registers(25000, 2).await.unwrap(),
        vec![0x1234, 0xabcd]
    );
    assert!(transport.read_holding_registers(25000, 2).await.is_err());
    assert_eq!(
        transport.read_holding_registers(25000, 2).await.unwrap(),
        vec![1, 2]
    );
    assert!(transport.read_holding_registers(25000, 126).await.is_err());
    server.await.unwrap();
}

#[tokio::test]
async fn tcp_timeout_discards_partial_connection() {
    use tokio::{io::AsyncReadExt, net::TcpListener};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut cfg = config(WordOrder::LowWordFirst);
    cfg.address = listener.local_addr().unwrap().to_string();
    cfg.timeout = Duration::from_millis(50);
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 12];
        stream.read_exact(&mut request).await.unwrap();
        // Không gửi response, chỉ giả lập trong loopback trên cổng ngẫu nhiên.
        tokio::time::sleep(Duration::from_secs(1)).await;
    });
    let mut transport = ModbusTcpReader::new(cfg);
    assert_eq!(
        transport
            .read_holding_registers(25000, 2)
            .await
            .unwrap_err(),
        As200Error::Timeout
    );
    server.abort();
}

#[tokio::test]
async fn metadata_conflicts_preserve_raw_without_advancing_tracker() {
    for order in [WordOrder::LowWordFirst, WordOrder::HighWordFirst] {
        for seq in [-1_i32, 999] {
            let mut memory = MemoryReader::new(order, 1);
            put32(&mut memory.words, 20 + 22, seq as u32, order);
            let raw = memory.words[20..260].to_vec();
            let mut reader = As200Reader::with_transport(config(order), memory);
            let report = reader.poll().await.unwrap();
            assert!(!report.diagnostics.is_empty());
            assert_eq!(report.buffers[0].raw_registers, raw);
            assert_eq!(report.control.total_count, 1);
            assert!(reader.tracker.total_count.is_none());
        }
        let mut reader = As200Reader::with_transport(config(order), MemoryReader::new(order, 42));
        assert!(
            reader
                .poll()
                .await
                .unwrap()
                .diagnostics
                .iter()
                .all(|d| !matches!(
                    d,
                    Diagnostic::InvalidRecord { .. } | Diagnostic::TrackerRejected { .. }
                ))
        );
        let mut memory = MemoryReader::new(order, 1);
        put32(&mut memory.words, 20 + 22, 0, order);
        let mut reader = As200Reader::with_transport(config(order), memory);
        assert!(
            reader
                .poll()
                .await
                .unwrap()
                .diagnostics
                .iter()
                .all(|d| !matches!(d, Diagnostic::InvalidRecord { .. }))
        );
    }
}

#[tokio::test]
async fn changed_raw_same_sequence_preserves_both_slots_and_old_checkpoint() {
    let order = WordOrder::LowWordFirst;
    let mut reader = As200Reader::with_transport(config(order), MemoryReader::new(order, 2));
    reader.poll().await.unwrap();
    reader.transport.words[20 + 2] ^= 1;
    let expected = reader.transport.words.clone();
    let report = reader.poll().await.unwrap();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| matches!(d, Diagnostic::TrackerRejected { .. }))
    );
    assert_eq!(reader.tracker.total_count, Some(2));
    assert_eq!(report.buffers[0].raw_registers, &expected[20..260]);
    assert_eq!(report.buffers[1].raw_registers, &expected[260..500]);
    assert!(report.new_records.is_empty());
}
