//! Công cụ chỉ đọc AS200, không chạy Tauri, PostgreSQL hay ACK.
#[path = "../plc/as200/mod.rs"]
mod as200;

use as200::{As200Config, WordOrder, model::PlcStickRecord, reader::As200Reader};
use std::time::Duration;

fn record_output(record: &PlcStickRecord, raw: bool) {
    println!(
        "Buffer={} WORD={} ProductCode={:?} StickNo={} RecordSeq={} Overall={:?} Status={:#06x}",
        record.buffer_index,
        record.raw_registers.len(),
        record.product_code,
        record.stick_no,
        record.record_seq,
        record.overall_result,
        record.status
    );
    for (name, station) in [("BUTT", &record.butt), ("TIP", &record.tip)] {
        println!(
            "  {name} Result={:?} PointCount={} NGPoint={} Load={} g",
            station.result, station.point_count, station.ng_point, station.load
        );
        for (index, point) in station.points.iter().enumerate() {
            println!(
                "    P{} Result={:?} AttemptCount={} DecideAttempt={} Position={} mm",
                index + 1,
                point.result,
                point.attempt_count,
                point.decide_attempt,
                point.position
            );
            for attempt in &point.attempts {
                println!(
                    "      A{} angle={} PushDist={} mm Bend={} mm{}",
                    attempt.number,
                    attempt.angle_degrees,
                    attempt.push_dist,
                    attempt.bend,
                    if i16::from(attempt.number) > point.attempt_count {
                        " (unused fixture)"
                    } else {
                        ""
                    }
                );
            }
        }
    }
    if raw {
        println!("  Raw240={:?}", record.raw_registers);
    }
}

async fn run() -> Result<(), String> {
    let mut config = As200Config {
        // Mặc định chỉ dành cho simulator; không khẳng định thông số của PLC thật.
        address: "127.0.0.1:1503".into(),
        unit_id: 1,
        modbus_address_offset: 0,
        word_order: WordOrder::LowWordFirst,
        timeout: Duration::from_secs(2),
    };
    let mut interval = Duration::from_millis(500);
    let (mut once, mut raw) = (false, false);
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        if argument == "--once" {
            once = true;
            continue;
        }
        if argument == "--raw" {
            raw = true;
            continue;
        }
        if argument == "--help" || argument == "-h" {
            println!(
                "as200-reader [--address 127.0.0.1:1503] [--unit 1] [--offset 0] [--word-order low|high] [--interval-ms 500] [--timeout-ms 2000] [--once] [--raw]"
            );
            return Ok(());
        }
        let value = args
            .next()
            .ok_or_else(|| format!("Thiếu giá trị {argument}"))?;
        match argument.as_str() {
            "--address" => config.address = value,
            "--unit" => config.unit_id = value.parse().map_err(|_| "Unit ID không hợp lệ")?,
            "--offset" => {
                config.modbus_address_offset = value.parse().map_err(|_| "Offset không hợp lệ")?
            }
            "--word-order" => {
                config.word_order = match value.as_str() {
                    "low" => WordOrder::LowWordFirst,
                    "high" => WordOrder::HighWordFirst,
                    _ => return Err("WordOrder phải là low hoặc high".into()),
                }
            }
            "--interval-ms" => {
                interval =
                    Duration::from_millis(value.parse().map_err(|_| "Interval không hợp lệ")?)
            }
            "--timeout-ms" => {
                config.timeout =
                    Duration::from_millis(value.parse().map_err(|_| "Timeout không hợp lệ")?)
            }
            _ => return Err(format!("Tùy chọn không hỗ trợ: {argument}")),
        }
    }
    if config.unit_id == 0 || config.unit_id > 247 || interval.is_zero() || config.timeout.is_zero()
    {
        return Err("Unit ID phải từ 1–247; interval và timeout phải lớn hơn 0".into());
    }
    println!(
        "AS200 read-only {} Unit={} WordOrder={:?} offset={}. Không lưu PostgreSQL, không ACK.",
        config.address, config.unit_id, config.word_order, config.modbus_address_offset
    );
    let mut reader = As200Reader::new(config);
    let mut previous_total = None;
    loop {
        match reader.poll_logged().await {
            Ok(report) => {
                if once
                    || previous_total != Some(report.control.total_count)
                    || !report.diagnostics.is_empty()
                {
                    println!(
                        "Control={}",
                        serde_json::to_string(&report.control).map_err(|e| e.to_string())?
                    );
                    println!(
                        "Observed new records={} (không có nghĩa đã lưu hoặc ACK)",
                        report.new_records.len()
                    );
                    for buffer in &report.buffers {
                        if let Some(record) = &buffer.record {
                            record_output(record, raw);
                        } else {
                            println!(
                                "Buffer={} WORD={} Status bit0=0; chưa có record hợp lệ",
                                buffer.buffer_index,
                                buffer.raw_registers.len()
                            );
                        }
                    }
                    previous_total = Some(report.control.total_count);
                }
                if report.diagnostics.iter().any(|d| {
                    matches!(
                        d,
                        as200::reader::Diagnostic::TrackerRejected {
                            session_unconfirmed: true,
                            ..
                        }
                    )
                }) {
                    return Err(as200::As200Error::SessionUnconfirmed.to_string());
                }
                if once {
                    return Ok(());
                }
            }
            Err(error) => {
                if once {
                    return Err(error.to_string());
                }
                if error == as200::As200Error::SessionUnconfirmed {
                    return Err("Counter/phiên PLC thay đổi. Xác nhận reset rồi chạy lại reader để lấy mốc mới.".into());
                }
                // Transport hủy socket lỗi và tự nối lại ở lượt polling sau.
            }
        }
        tokio::time::sleep(interval).await;
    }
}

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Tokio runtime");
    if let Err(error) = runtime.block_on(run()) {
        eprintln!("AS200 reader: {error}");
        std::process::exit(1);
    }
}
