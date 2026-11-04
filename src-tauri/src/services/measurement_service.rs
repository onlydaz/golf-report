use crate::enums::{MeasurementResult, TipPositionResult};
use crate::models::{calculate_overall_result, calculate_tip_result, CreateMeasurementRequest, Measurement, MeasurementListRequest, MeasurementListResponse, MeasurementStatistics};
use crate::repositories::MeasurementRepository;
use chrono::Utc;
use sqlx::PgPool;
use sqlx::types::BigDecimal;

pub struct MeasurementService;

impl MeasurementService {
    pub fn validate(request: &CreateMeasurementRequest) -> Result<(MeasurementResult, MeasurementResult), String> {
        if request.product_code.trim().is_empty() {
            return Err("Mã sản phẩm là bắt buộc.".to_string());
        }
        if request.club_number.trim().is_empty() {
            return Err("Số thứ tự gậy là bắt buộc.".to_string());
        }
        if request.weight < BigDecimal::from(0) {
            return Err("Weight phải là số không âm.".to_string());
        }
        if request.measured_at > Utc::now() {
            return Err("Thời gian đo không được ở tương lai.".to_string());
        }
        if request.tip_positions.len() != 4 {
            return Err("Phải có đúng 4 vị trí Tip.".to_string());
        }
        for (index, tip) in request.tip_positions.iter().enumerate() {
            if tip.position_number != (index + 1) as i16 {
                return Err("Các vị trí Tip phải liên tiếp từ 1 đến 4.".to_string());
            }
            match tip.result {
                TipPositionResult::Skipped => {
                    if tip.final_distance.is_some() || tip.retry_count.is_some() {
                        return Err(format!("Tip {} bị SKIPPED không được có dữ liệu.", index + 1));
                    }
                }
                TipPositionResult::Ok | TipPositionResult::Ng => {
                    let distance = tip.final_distance.as_ref().ok_or_else(|| format!("Tip {} thiếu khoảng cách cuối.", index + 1))?;
                    if distance < &BigDecimal::from(0) {
                        return Err(format!("Khoảng cách Tip {} phải là số không âm.", index + 1));
                    }
                    let retry = tip.retry_count.ok_or_else(|| format!("Tip {} thiếu số lần kiểm tra lại.", index + 1))?;
                    if !(0..=4).contains(&retry) {
                        return Err(format!("Số lần kiểm tra lại của Tip {} phải từ 0 đến 4.", index + 1));
                    }
                    if tip.result == TipPositionResult::Ng && retry != 4 {
                        return Err(format!("Tip {} có kết quả NG phải kiểm tra lại đủ 4 lần.", index + 1));
                    }
                }
            }
            if request.tip_positions[..index].iter().any(|previous| previous.result == TipPositionResult::Ng)
                && tip.result != TipPositionResult::Skipped {
                return Err("Các vị trí Tip phía sau NG phải là SKIPPED.".to_string());
            }
        }
        if request.butt_distance < BigDecimal::from(0) {
            return Err("Khoảng cách Butt phải là số không âm.".to_string());
        }
        if request.butt_result == MeasurementResult::Incomplete {
            return Err("Kết quả Butt là bắt buộc.".to_string());
        }
        let tip_result = calculate_tip_result(&request.tip_positions);
        let overall_result = calculate_overall_result(tip_result, request.butt_result);
        if tip_result == MeasurementResult::Incomplete || overall_result == MeasurementResult::Incomplete {
            return Err("Dữ liệu đo chưa hoàn chỉnh.".to_string());
        }
        Ok((tip_result, overall_result))
    }

    pub async fn create(pool: &PgPool, request: CreateMeasurementRequest) -> Result<i64, String> {
        let (tip_result, overall_result) = Self::validate(&request)?;
        let mut transaction = pool.begin().await.map_err(|_| "Không thể bắt đầu giao dịch lưu dữ liệu.".to_string())?;
        let id = MeasurementRepository::insert(
            &mut transaction,
            &request.product_code,
            &request.club_number,
            request.measured_at,
            &request.weight,
            tip_result,
            request.butt_result,
            &request.butt_distance,
            overall_result,
            &request.tip_positions,
        ).await?;
        transaction.commit().await.map_err(|_| "Không thể hoàn tất giao dịch lưu dữ liệu.".to_string())?;
        Ok(id)
    }

    pub async fn list(pool: &PgPool, request: MeasurementListRequest) -> Result<MeasurementListResponse, String> {
        MeasurementRepository::list(pool, &request).await
    }

    pub async fn find_by_id(pool: &PgPool, id: i64) -> Result<Measurement, String> {
        MeasurementRepository::find_by_id(pool, id).await?.ok_or_else(|| "Không tìm thấy phép đo.".to_string())
    }

    pub async fn statistics(pool: &PgPool, request: MeasurementListRequest) -> Result<MeasurementStatistics, String> {
        MeasurementRepository::statistics(pool, &request).await
    }
}
