use crate::enums::{MeasurementResult, TipPositionResult};
use crate::models::{DailyMeasurementStatistics, Measurement, MeasurementListRequest, MeasurementListResponse, MeasurementStatistics, ResultDistribution, StationMeasurementStatistics, TipPosition};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use sqlx::{postgres::PgRow, FromRow, PgPool, Postgres, QueryBuilder, Row};

#[derive(Debug, FromRow)]
struct MeasurementRow {
    id: i64,
    product_code: String,
    club_number: String,
    measured_at: DateTime<Utc>,
    weight: BigDecimal,
    tip_result: String,
    butt_result: String,
    butt_distance: BigDecimal,
    overall_result: String,
}

#[derive(Debug, FromRow)]
struct TipPositionRow {
    position_number: i16,
    final_distance: Option<BigDecimal>,
    retry_count: Option<i16>,
    result: String,
}

fn measurement_result(value: &str) -> Result<MeasurementResult, String> {
    match value {
        "OK" => Ok(MeasurementResult::Ok),
        "NG" => Ok(MeasurementResult::Ng),
        "INCOMPLETE" => Ok(MeasurementResult::Incomplete),
        _ => Err("Cơ sở dữ liệu chứa kết quả không hợp lệ.".to_string()),
    }
}

fn tip_result(value: &str) -> Result<TipPositionResult, String> {
    match value {
        "OK" => Ok(TipPositionResult::Ok),
        "NG" => Ok(TipPositionResult::Ng),
        "SKIPPED" => Ok(TipPositionResult::Skipped),
        _ => Err("Cơ sở dữ liệu chứa kết quả Tip không hợp lệ.".to_string()),
    }
}

impl MeasurementRow {
    fn into_measurement(self, tips: Vec<TipPosition>) -> Result<Measurement, String> {
        Ok(Measurement {
            id: self.id,
            product_code: self.product_code,
            club_number: self.club_number,
            measured_at: self.measured_at,
            weight: self.weight,
            tip_positions: tips,
            tip_result: measurement_result(&self.tip_result)?,
            butt_distance: self.butt_distance,
            butt_result: measurement_result(&self.butt_result)?,
            overall_result: measurement_result(&self.overall_result)?,
        })
    }
}

pub struct MeasurementRepository;

impl MeasurementRepository {
    pub async fn insert<'a>(
        transaction: &mut sqlx::Transaction<'a, Postgres>,
        product_code: &str,
        club_number: &str,
        measured_at: DateTime<Utc>,
        weight: &BigDecimal,
        tip_result: MeasurementResult,
        butt_result: MeasurementResult,
        butt_distance: &BigDecimal,
        overall_result: MeasurementResult,
        tips: &[TipPosition],
    ) -> Result<i64, String> {
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO measurements (product_code, club_number, measured_at, weight, tip_result, butt_result, butt_distance, overall_result)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
        )
        .bind(product_code)
        .bind(club_number)
        .bind(measured_at)
        .bind(weight)
        .bind(tip_result.as_str())
        .bind(butt_result.as_str())
        .bind(butt_distance)
        .bind(overall_result.as_str())
        .fetch_one(&mut **transaction)
        .await
        .map_err(|_| "Không thể lưu thông tin phép đo.".to_string())?;

        for tip in tips {
            sqlx::query(
                "INSERT INTO tip_positions (measurement_id, position_number, final_distance, retry_count, result)
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(row.0)
            .bind(tip.position_number)
            .bind(&tip.final_distance)
            .bind(tip.retry_count)
            .bind(tip.result.as_str())
            .execute(&mut **transaction)
            .await
            .map_err(|_| "Không thể lưu dữ liệu các vị trí Tip.".to_string())?;
        }
        Ok(row.0)
    }

    async fn tips(pool: &PgPool, measurement_id: i64) -> Result<Vec<TipPosition>, String> {
        let rows = sqlx::query_as::<_, TipPositionRow>(
            "SELECT position_number, final_distance, retry_count, result
             FROM tip_positions WHERE measurement_id = $1 ORDER BY position_number",
        )
        .bind(measurement_id)
        .fetch_all(pool)
        .await
        .map_err(|_| "Không thể truy xuất dữ liệu các vị trí Tip.".to_string())?;
        rows.into_iter()
            .map(|row| Ok(TipPosition {
                position_number: row.position_number,
                final_distance: row.final_distance,
                retry_count: row.retry_count,
                result: tip_result(&row.result)?,
            }))
            .collect()
    }

    pub async fn find_by_id(pool: &PgPool, id: i64) -> Result<Option<Measurement>, String> {
        let row = sqlx::query_as::<_, MeasurementRow>(
            "SELECT id, product_code, club_number, measured_at, weight, tip_result, butt_result, butt_distance, overall_result
             FROM measurements WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|_| "Không thể truy xuất phép đo.".to_string())?;
        match row {
            Some(row) => Ok(Some(row.into_measurement(Self::tips(pool, id).await?)?)),
            None => Ok(None),
        }
    }

    pub async fn list(pool: &PgPool, request: &MeasurementListRequest) -> Result<MeasurementListResponse, String> {
        let page = request.page.unwrap_or(1).max(1);
        let page_size = request.page_size.unwrap_or(20).clamp(1, 100);
        let offset = (page - 1) * page_size;
        let mut count_query = QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM measurements WHERE 1=1");
        let mut data_query = QueryBuilder::<Postgres>::new(
            "SELECT id, product_code, club_number, measured_at, weight, tip_result, butt_result, butt_distance, overall_result FROM measurements WHERE 1=1",
        );
        if let Some(value) = request.search.as_deref() {
            let pattern = format!("%{}%", value);
            count_query.push(" AND (product_code ILIKE ").push_bind(&pattern).push(" OR club_number ILIKE ").push_bind(&pattern).push(")");
            data_query.push(" AND (product_code ILIKE ").push_bind(&pattern).push(" OR club_number ILIKE ").push_bind(&pattern).push(")");
        }
        if let Some(value) = request.product_code.as_deref() {
            count_query.push(" AND product_code = ").push_bind(value);
            data_query.push(" AND product_code = ").push_bind(value);
        }
        if let Some(value) = request.club_number.as_deref() {
            count_query.push(" AND club_number = ").push_bind(value);
            data_query.push(" AND club_number = ").push_bind(value);
        }
        if let Some(value) = request.measured_from {
            count_query.push(" AND measured_at >= ").push_bind(value);
            data_query.push(" AND measured_at >= ").push_bind(value);
        }
        if let Some(value) = request.measured_to {
            count_query.push(" AND measured_at <= ").push_bind(value);
            data_query.push(" AND measured_at <= ").push_bind(value);
        }
        if let Some(value) = request.tip_result {
            count_query.push(" AND tip_result = ").push_bind(value.as_str());
            data_query.push(" AND tip_result = ").push_bind(value.as_str());
        }
        if let Some(value) = request.butt_result {
            count_query.push(" AND butt_result = ").push_bind(value.as_str());
            data_query.push(" AND butt_result = ").push_bind(value.as_str());
        }
        if let Some(value) = request.result {
            count_query.push(" AND overall_result = ").push_bind(value.as_str());
            data_query.push(" AND overall_result = ").push_bind(value.as_str());
        }
        let (total,): (i64,) = count_query.build_query_as().fetch_one(pool).await
            .map_err(|_| "Không thể đếm danh sách phép đo.".to_string())?;
        let rows: Vec<MeasurementRow> = data_query
            .push(" ORDER BY measured_at DESC LIMIT ").push_bind(page_size)
            .push(" OFFSET ").push_bind(offset)
            .build_query_as()
            .fetch_all(pool)
            .await
            .map_err(|_| "Không thể truy xuất danh sách phép đo.".to_string())?;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            let id = row.id;
            items.push(row.into_measurement(Self::tips(pool, id).await?)?);
        }
        Ok(MeasurementListResponse { items, page, page_size, total })
    }

    pub async fn statistics(pool: &PgPool, request: &MeasurementListRequest) -> Result<MeasurementStatistics, String> {
        let mut summary_query = QueryBuilder::<Postgres>::new(
            "SELECT COUNT(*) AS total,
                    COUNT(*) FILTER (WHERE overall_result = 'OK') AS total_ok,
                    COUNT(*) FILTER (WHERE overall_result = 'NG') AS total_ng,
                    COUNT(*) FILTER (WHERE tip_result = 'NG') AS tip_ng,
                    COUNT(*) FILTER (WHERE butt_result = 'NG') AS butt_ng
             FROM measurements WHERE 1=1",
        );
        let mut daily_query = QueryBuilder::<Postgres>::new(
            "SELECT measured_at::date::text AS date, COUNT(*) AS total,
                    COUNT(*) FILTER (WHERE overall_result = 'OK') AS ok,
                    COUNT(*) FILTER (WHERE overall_result = 'NG') AS ng
             FROM measurements WHERE 1=1",
        );
        let mut distribution_query = QueryBuilder::<Postgres>::new(
            "SELECT overall_result AS result, COUNT(*) AS count
             FROM measurements WHERE 1=1",
        );
        let mut station_query = QueryBuilder::<Postgres>::new(
            "SELECT 'Tip' AS station,
                    COUNT(*) FILTER (WHERE tip_result = 'OK') AS ok,
                    COUNT(*) FILTER (WHERE tip_result = 'NG') AS ng
             FROM measurements WHERE 1=1",
        );
        let add_filter = |query: &mut QueryBuilder<Postgres>, request: &MeasurementListRequest| {
            if let Some(value) = request.search.as_deref() {
                let pattern = format!("%{}%", value);
                query.push(" AND (product_code ILIKE ").push_bind(pattern.clone()).push(" OR club_number ILIKE ").push_bind(pattern).push(")");
            }
            if let Some(value) = request.product_code.as_deref() { query.push(" AND product_code = ").push_bind(value); }
            if let Some(value) = request.club_number.as_deref() { query.push(" AND club_number = ").push_bind(value); }
            if let Some(value) = request.measured_from { query.push(" AND measured_at >= ").push_bind(value); }
            if let Some(value) = request.measured_to { query.push(" AND measured_at <= ").push_bind(value); }
            if let Some(value) = request.tip_result { query.push(" AND tip_result = ").push_bind(value.as_str()); }
            if let Some(value) = request.butt_result { query.push(" AND butt_result = ").push_bind(value.as_str()); }
            if let Some(value) = request.result { query.push(" AND overall_result = ").push_bind(value.as_str()); }
        };
        add_filter(&mut summary_query, request);
        add_filter(&mut daily_query, request);
        add_filter(&mut distribution_query, request);
        add_filter(&mut station_query, request);
        daily_query.push(" GROUP BY measured_at::date ORDER BY measured_at::date");
        distribution_query.push(" GROUP BY overall_result ORDER BY overall_result");
        let mut butt_query = QueryBuilder::<Postgres>::new(
            "SELECT COUNT(*) FILTER (WHERE butt_result = 'OK') AS ok,
                    COUNT(*) FILTER (WHERE butt_result = 'NG') AS ng
             FROM measurements WHERE 1=1",
        );
        add_filter(&mut butt_query, request);
        let row: PgRow = summary_query.build().fetch_one(pool).await
            .map_err(|_| "Không thể truy xuất thống kê phép đo.".to_string())?;
        let daily_rows: Vec<PgRow> = daily_query.build().fetch_all(pool).await
            .map_err(|_| "Không thể truy xuất thống kê theo ngày.".to_string())?;
        let distribution_rows: Vec<PgRow> = distribution_query.build().fetch_all(pool).await
            .map_err(|_| "Không thể truy xuất phân bố kết quả.".to_string())?;
        let tip_row: PgRow = station_query.build().fetch_one(pool).await
            .map_err(|_| "Không thể truy xuất thống kê trạm Tip.".to_string())?;
        let butt_row: PgRow = butt_query.build().fetch_one(pool).await
            .map_err(|_| "Không thể truy xuất thống kê trạm Butt.".to_string())?;
        let total: i64 = row.try_get("total").map_err(|_| "Không thể đọc thống kê phép đo.".to_string())?;
        let total_ok: i64 = row.try_get("total_ok").map_err(|_| "Không thể đọc thống kê phép đo.".to_string())?;
        let total_ng: i64 = row.try_get("total_ng").map_err(|_| "Không thể đọc thống kê phép đo.".to_string())?;
        let tip_ng: i64 = row.try_get("tip_ng").map_err(|_| "Không thể đọc thống kê Tip.".to_string())?;
        let butt_ng: i64 = row.try_get("butt_ng").map_err(|_| "Không thể đọc thống kê Butt.".to_string())?;
        let daily = daily_rows.into_iter().map(|item| Ok(DailyMeasurementStatistics {
            date: item.try_get("date").map_err(|_| "Không thể đọc ngày thống kê.".to_string())?,
            total: item.try_get("total").map_err(|_| "Không thể đọc thống kê theo ngày.".to_string())?,
            ok: item.try_get("ok").map_err(|_| "Không thể đọc thống kê theo ngày.".to_string())?,
            ng: item.try_get("ng").map_err(|_| "Không thể đọc thống kê theo ngày.".to_string())?,
        })).collect::<Result<Vec<_>, String>>()?;
        let overall_distribution = distribution_rows.into_iter().map(|item| Ok(ResultDistribution {
            result: item.try_get("result").map_err(|_| "Không thể đọc phân bố kết quả.".to_string())?,
            count: item.try_get("count").map_err(|_| "Không thể đọc phân bố kết quả.".to_string())?,
        })).collect::<Result<Vec<_>, String>>()?;
        let tip_ok: i64 = tip_row.try_get("ok").map_err(|_| "Không thể đọc thống kê Tip.".to_string())?;
        let butt_ok: i64 = butt_row.try_get("ok").map_err(|_| "Không thể đọc thống kê Butt.".to_string())?;
        Ok(MeasurementStatistics {
            total_measurements: total,
            total_ok,
            total_ng,
            pass_rate: if total == 0 { 0.0 } else { (total_ok as f64 / total as f64) * 100.0 },
            tip_ng,
            butt_ng,
            daily,
            station_results: vec![
                StationMeasurementStatistics { station: "Tip".to_string(), ok: tip_ok, ng: tip_ng },
                StationMeasurementStatistics { station: "Butt".to_string(), ok: butt_ok, ng: butt_ng },
            ],
            overall_distribution,
        })
    }
}
