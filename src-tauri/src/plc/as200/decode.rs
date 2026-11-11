use super::{
    As200Error,
    map::{self, control as c, record as r},
    model::*,
};

fn invalid(message: impl Into<String>) -> As200Error {
    As200Error::InvalidData(message.into())
}

pub fn uint16(words: &[u16], offset: usize) -> Result<u16, As200Error> {
    words
        .get(offset)
        .copied()
        .ok_or_else(|| invalid(format!("Thiếu WORD tại offset {offset}")))
}
pub fn int16(words: &[u16], offset: usize) -> Result<i16, As200Error> {
    Ok(uint16(words, offset)? as i16)
}
fn bits32(words: &[u16], offset: usize, order: WordOrder) -> Result<u32, As200Error> {
    let a = u32::from(uint16(words, offset)?);
    let b = u32::from(uint16(
        words,
        offset
            .checked_add(1)
            .ok_or_else(|| invalid("Offset tràn"))?,
    )?);
    Ok(match order {
        WordOrder::HighWordFirst => (a << 16) | b,
        WordOrder::LowWordFirst => (b << 16) | a,
    })
}
pub fn dint32(words: &[u16], offset: usize, order: WordOrder) -> Result<i32, As200Error> {
    Ok(bits32(words, offset, order)? as i32)
}
pub fn real32(words: &[u16], offset: usize, order: WordOrder) -> Result<f32, As200Error> {
    let value = f32::from_bits(bits32(words, offset, order)?);
    if !value.is_finite() {
        return Err(invalid(format!("REAL32 NaN/Infinity tại offset {offset}")));
    }
    Ok(value) // REAL32 đúng đơn vị tài liệu, không chia 100.
}
pub fn product_code(words: &[u16]) -> Result<String, As200Error> {
    if words.len() != r::PRODUCT_WORDS {
        return Err(invalid("ProductCode phải gồm 10 WORD"));
    }
    let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    if !bytes[..end].is_ascii() || bytes[end..].iter().any(|byte| *byte != 0) {
        return Err(invalid(
            "ProductCode phải là ASCII, padding bằng 0 sau ký tự NUL đầu tiên",
        ));
    }
    String::from_utf8(bytes[..end].to_vec()).map_err(|_| invalid("ProductCode không hợp lệ"))
}
fn result(value: i16) -> Result<PlcResult, As200Error> {
    match value {
        0 => Ok(PlcResult::Unmeasured),
        1 => Ok(PlcResult::Ok),
        2 => Ok(PlcResult::Ng),
        _ => Err(invalid(format!("Mã kết quả không hợp lệ: {value}"))),
    }
}
fn array<T, const N: usize>(items: Vec<T>) -> Result<[T; N], As200Error> {
    items.try_into().map_err(|_| invalid("Sai kích thước mảng"))
}

pub fn control(words: &[u16], order: WordOrder) -> Result<PlcControl, As200Error> {
    if words.len() != map::CONTROL_SIZE {
        return Err(invalid("Control cần đúng 20 WORD"));
    }
    let value = PlcControl {
        format_version: int16(words, c::FORMAT_VERSION)?,
        record_size: int16(words, c::RECORD_SIZE)?,
        buffer_count: int16(words, c::BUFFER_COUNT)?,
        write_idx: int16(words, c::WRITE_IDX)?,
        total_count: dint32(words, c::TOTAL_COUNT, order)?,
        last_idx: int16(words, c::LAST_IDX)?,
        heartbeat: int16(words, c::HEARTBEAT)?,
        pc_read_count: dint32(words, c::PC_READ_COUNT, order)?,
        pc_heartbeat: int16(words, c::PC_HEARTBEAT)?,
        data_ready: int16(words, c::DATA_READY)?,
        pc_read_done: int16(words, c::PC_READ_DONE)?,
        reserve: words[13..20].try_into().unwrap(),
    };
    if value.format_version != map::FORMAT_VERSION
        || value.record_size != map::RECORD_SIZE as i16
        || value.buffer_count != map::BUFFER_COUNT as i16
    {
        return Err(invalid(
            "FormatVersion/RecordSize/BufferCount không khớp map v1/240/2",
        ));
    }
    if !(0..2).contains(&value.write_idx)
        || !(0..2).contains(&value.last_idx)
        || !(0..=1).contains(&value.data_ready)
        || !(0..=1).contains(&value.pc_read_done)
        || value.heartbeat < 0
    {
        return Err(invalid(
            "Chỉ số buffer, heartbeat hoặc cờ điều khiển ngoài phạm vi",
        ));
    }
    Ok(value)
}

fn point(words: &[u16], base: usize, order: WordOrder) -> Result<PlcMeasurementPoint, As200Error> {
    let point_result = result(int16(words, base)?)?;
    let count = int16(words, base + 1)?;
    let decide = int16(words, base + 2)?;
    if !(0..=4).contains(&count)
        || !(0..=count).contains(&decide)
        || (point_result != PlcResult::Unmeasured && (count == 0 || decide == 0))
    {
        return Err(invalid(format!(
            "AttemptCount/DecideAttempt không hợp lệ tại offset {base}"
        )));
    }
    let mut attempts = Vec::new();
    for index in 0..r::ATTEMPT_COUNT {
        attempts.push(PlcMeasurementAttempt {
            number: (index + 1) as u8,
            angle_degrees: index as u16 * 90,
            push_dist: real32(words, base + 6 + index * 4, order)?,
            bend: real32(words, base + 8 + index * 4, order)?,
        });
    }
    Ok(PlcMeasurementPoint {
        result: point_result,
        attempt_count: count,
        decide_attempt: decide,
        reserve: uint16(words, base + 3)?,
        position: real32(words, base + 4, order)?,
        attempts: array(attempts)?,
    })
}
fn station(
    words: &[u16],
    base: usize,
    points_base: usize,
    order: WordOrder,
) -> Result<PlcStation, As200Error> {
    let station_result = result(int16(words, base)?)?;
    let point_count = int16(words, base + 1)?;
    let ng_point = int16(words, base + 2)?;
    if !(0..=4).contains(&point_count)
        || !(0..=4).contains(&ng_point)
        || (station_result == PlcResult::Ok && ng_point != 0)
        || (station_result == PlcResult::Ng && ng_point == 0)
    {
        return Err(invalid(format!(
            "PointCount/NGPoint không hợp lệ tại offset {base}"
        )));
    }
    let mut points = Vec::new();
    for number in 1..=r::POINT_COUNT {
        points.push(point(
            words,
            map::point_offset(points_base, number).unwrap(),
            order,
        )?);
    }
    Ok(PlcStation {
        result: station_result,
        point_count,
        ng_point,
        reserve: uint16(words, base + 3)?,
        load: real32(words, base + 4, order)?,
        reserve_after_load: [uint16(words, base + 6)?, uint16(words, base + 7)?],
        points: array(points)?,
    })
}

pub fn record(
    words: &[u16],
    buffer_index: usize,
    order: WordOrder,
) -> Result<PlcStickRecord, As200Error> {
    if words.len() != map::RECORD_SIZE || buffer_index >= map::BUFFER_COUNT {
        return Err(invalid("Record cần 240 WORD và buffer_index 0 hoặc 1"));
    }
    let version = int16(words, r::VERSION)?;
    if version != map::FORMAT_VERSION {
        return Err(invalid(format!("Record Version không hỗ trợ: {version}")));
    }
    let status = uint16(words, r::STATUS)?;
    Ok(PlcStickRecord {
        buffer_index,
        version,
        status,
        valid_unacknowledged: status & 1 != 0,
        stick_no: dint32(words, r::STICK_NO, order)?,
        product_code: product_code(&words[r::PRODUCT_CODE..r::PRODUCT_CODE + r::PRODUCT_WORDS])?,
        unknown_header_14_19: words[r::UNKNOWN_HEADER..r::OVERALL_RESULT]
            .try_into()
            .unwrap(),
        overall_result: result(int16(words, r::OVERALL_RESULT)?)?,
        reserve_header: words[21],
        record_seq: dint32(words, r::SEQUENCE, order)?,
        butt: station(words, r::BUTT, r::BUTT_POINTS, order)?,
        tip: station(words, r::TIP, r::TIP_POINTS, order)?,
        reserve_end: words[216..240].to_vec(),
        raw_registers: words.to_vec(),
    })
}
