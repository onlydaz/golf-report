use super::register_map::*;
use crate::{
    enums::{MeasurementResult, TipPositionResult},
    models::{CreateMeasurementRequest, TipPosition},
    services::MeasurementService,
};
use bigdecimal::BigDecimal;
use chrono::Utc;

pub fn decode(raw: &[u16; COUNT], product: &str) -> Result<CreateMeasurementRequest, String> {
    if raw[COMPLETED] != 1 {
        return Err("Phép đo chưa hoàn thành.".into());
    }
    let scaled = |value: u16| BigDecimal::new(value.into(), 2);
    let mut tips = Vec::new();
    for i in 0..4 {
        let result = match raw[TIP_RESULT + i] {
            1 => TipPositionResult::Ok,
            2 => TipPositionResult::Ng,
            3 => TipPositionResult::Skipped,
            _ => return Err(format!("Mã kết quả Tip {} không hợp lệ.", i + 1)),
        };
        let skipped = result == TipPositionResult::Skipped;
        tips.push(TipPosition {
            position_number: (i + 1) as i16,
            final_distance: if skipped {
                None
            } else {
                Some(scaled(raw[TIP_DISTANCE + i]))
            },
            retry_count: if skipped {
                None
            } else {
                Some(i16::try_from(raw[TIP_RETRY + i]).map_err(|_| "retry_count không hợp lệ.")?)
            },
            result,
        });
    }
    let request = CreateMeasurementRequest {
        product_code: product.into(),
        club_number: raw[CLUB].to_string(),
        measured_at: Utc::now(),
        weight: scaled(raw[WEIGHT]),
        tip_positions: tips,
        butt_distance: scaled(raw[BUTT_DISTANCE]),
        butt_result: match raw[BUTT_RESULT] {
            1 => MeasurementResult::Ok,
            2 => MeasurementResult::Ng,
            _ => return Err("Kết quả Butt không hợp lệ.".into()),
        },
    };
    MeasurementService::validate(&request)?;
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn raw() -> [u16; COUNT] {
        [
            1, 1, 2, 12345, 1001, 1100, 1200, 1300, 0, 1, 2, 4, 1, 1, 1, 1, 900, 1,
        ]
    }
    #[test]
    fn ok_and_butt_ng() {
        let mut r = raw();
        let request = decode(&r, "P").unwrap();
        assert_eq!(request.weight, BigDecimal::new(12345.into(), 2));
        assert_eq!(
            MeasurementService::validate(&request).unwrap().1,
            MeasurementResult::Ok
        );
        r[17] = 2;
        assert_eq!(
            MeasurementService::validate(&decode(&r, "P").unwrap())
                .unwrap()
                .1,
            MeasurementResult::Ng
        );
    }
    #[test]
    fn skipped_null_and_invalid_ng() {
        let mut r = raw();
        r[13] = 2;
        r[9] = 4;
        r[14] = 3;
        r[15] = 3;
        let request = decode(&r, "P").unwrap();
        assert!(request.tip_positions[2].final_distance.is_none());
        assert!(request.tip_positions[2].retry_count.is_none());
        r[9] = 3;
        assert!(decode(&r, "P").is_err());
        assert!(decode(&raw(), "").is_err());
    }
}
