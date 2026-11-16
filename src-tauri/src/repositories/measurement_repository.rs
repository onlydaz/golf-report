use crate::models::*;
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};

const JOINS: &str = " FROM measurements m LEFT JOIN measurement_stations t ON t.measurement_id=m.id AND t.station_type='TIP' LEFT JOIN measurement_stations b ON b.measurement_id=m.id AND b.station_type='BUTT' WHERE 1=1";
fn filter(q: &mut QueryBuilder<Postgres>, request: &MeasurementListRequest) {
    if let Some(value) = request.search.as_deref().filter(|value| !value.is_empty()) {
        let pattern = format!("%{value}%");
        q.push(" AND (m.product_code ILIKE ")
            .push_bind(pattern.clone())
            .push(" OR m.club_number::text ILIKE ")
            .push_bind(pattern)
            .push(")");
    }
    if let Some(value) = &request.product_code {
        q.push(" AND m.product_code=").push_bind(value);
    }
    if let Some(value) = request.club_number {
        q.push(" AND m.club_number=").push_bind(value);
    }
    if let Some(value) = request.measured_from {
        q.push(" AND m.measured_at>=").push_bind(value);
    }
    if let Some(value) = request.measured_to {
        q.push(" AND m.measured_at<").push_bind(value);
    }
    if let Some(value) = request.result {
        q.push(" AND m.overall_result=").push_bind(value.as_str());
    }
    if let Some(value) = request.tip_result {
        q.push(" AND t.result=").push_bind(value.as_str());
    }
    if let Some(value) = request.butt_result {
        q.push(" AND b.result=").push_bind(value.as_str());
    }
}

#[cfg(test)]
mod report_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "Opt-in AS200_TEST_DATABASE=golf_report; SELECT only, no test data created"]
    async fn postgres_report_filters_pagination_and_nested_details_read_only() {
        use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
        let target = std::env::var("AS200_TEST_DATABASE").expect("Specify development database");
        assert_eq!(target, "golf_report");
        let config = crate::database::config::load_config().unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(
                PgConnectOptions::new()
                    .host(&config.host)
                    .port(config.port)
                    .database(&target)
                    .username(&config.username)
                    .password(&config.password)
                    .options([("default_transaction_read_only", "on")]),
            )
            .await
            .unwrap();
        let all = MeasurementRepository::report(&pool, &MeasurementListRequest::default())
            .await
            .unwrap();
        let boundary: String = sqlx::query_scalar("SELECT (TIMESTAMPTZ '2026-11-09 17:30:00+00' AT TIME ZONE 'Asia/Ho_Chi_Minh')::date::text")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(boundary, "2026-11-10");
        let included:Vec<bool> = sqlx::query_scalar("SELECT instant >= TIMESTAMPTZ '2026-11-10 00:00:00+07' AND instant < TIMESTAMPTZ '2026-11-11 00:00:00+07' FROM (VALUES (TIMESTAMPTZ '2026-11-10 23:59:59.999000+07'),(TIMESTAMPTZ '2026-11-10 23:59:59.999500+07'),(TIMESTAMPTZ '2026-11-11 00:00:00.000000+07')) AS samples(instant)")
            .fetch_all(&pool).await.unwrap();
        assert_eq!(included, [true, true, false]);
        let multi:bool=sqlx::query_scalar("SELECT TIMESTAMPTZ '2026-11-12 23:59:59.999500+07' >= TIMESTAMPTZ '2026-11-10 00:00:00+07' AND TIMESTAMPTZ '2026-11-12 23:59:59.999500+07' < TIMESTAMPTZ '2026-11-13 00:00:00+07'")
            .fetch_one(&pool).await.unwrap();
        assert!(multi);

        let daily = MeasurementRepository::statistics(&pool, &MeasurementListRequest::default())
            .await
            .unwrap()
            .daily;
        let mut expected = std::collections::BTreeMap::<String, i64>::new();
        for item in &all {
            let date = item
                .summary
                .measured_at
                .with_timezone(&chrono::FixedOffset::east_opt(7 * 3600).unwrap())
                .format("%Y-%m-%d")
                .to_string();
            *expected.entry(date).or_default() += 1;
        }
        assert_eq!(
            daily
                .iter()
                .map(|d| (d.date.clone(), d.total))
                .collect::<std::collections::BTreeMap<_, _>>(),
            expected
        );
        for pair in all.windows(2) {
            assert!(
                (pair[0].summary.measured_at, pair[0].summary.id)
                    >= (pair[1].summary.measured_at, pair[1].summary.id)
            );
        }
        for item in &all {
            let detail = MeasurementRepository::find_by_id(&pool, item.summary.id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                serde_json::to_value(item).unwrap(),
                serde_json::to_value(detail).unwrap()
            );
            assert_eq!(item.stations.len(), 2);
            for station in &item.stations {
                assert_eq!(station.points.len(), 4);
                for point in &station.points {
                    assert_eq!(point.attempts.len(), point.attempt_count as usize);
                }
            }
        }
        if let Some(first) = all.first() {
            let request = MeasurementListRequest {
                product_code: Some(first.summary.product_code.clone()),
                club_number: Some(first.summary.club_number),
                result: Some(first.summary.overall_result),
                measured_from: Some(first.summary.measured_at),
                measured_to: Some(first.summary.measured_at + chrono::Duration::microseconds(1)),
                page_size: Some(1),
                ..Default::default()
            };
            let filtered = MeasurementRepository::report(&pool, &request)
                .await
                .unwrap();
            assert!(
                filtered
                    .iter()
                    .any(|item| item.summary.id == first.summary.id)
            );
            assert!(filtered.iter().all(|item| item.summary.product_code
                == first.summary.product_code
                && item.summary.club_number == first.summary.club_number
                && item.summary.overall_result == first.summary.overall_result
                && item.summary.measured_at == first.summary.measured_at));
            let list = MeasurementRepository::list(&pool, &request).await.unwrap();
            assert_eq!(list.total as usize, filtered.len());
            assert_eq!(list.items.len(), 1);
            let stats = MeasurementRepository::statistics(&pool, &request)
                .await
                .unwrap();
            assert_eq!(stats.total_measurements as usize, filtered.len());
        }
        let empty = MeasurementListRequest {
            measured_from: Some(chrono::DateTime::from_timestamp(0, 0).unwrap()),
            measured_to: Some(chrono::DateTime::from_timestamp(0, 0).unwrap()),
            ..Default::default()
        };
        assert!(
            MeasurementRepository::report(&pool, &empty)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            MeasurementRepository::statistics(&pool, &empty)
                .await
                .unwrap()
                .total_measurements,
            0
        );
        println!(
            "Read-only AS200 report: {} measurements checked against nested detail; filters and empty results verified",
            all.len()
        );
        pool.close().await;
    }
}
pub struct MeasurementRepository;
impl MeasurementRepository {
    pub async fn insert(
        tx: &mut Transaction<'_, Postgres>,
        input: &MeasurementDraft,
    ) -> Result<i64, String> {
        let (id,):(i64,)=sqlx::query_as("INSERT INTO measurements(source_id,session_id,record_seq,total_count,product_code,club_number,measured_at,overall_result) VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id")
            .bind(input.source_id).bind(&input.confirmed_session_id).bind(input.record_seq).bind(input.total_count).bind(&input.product_code).bind(input.club_number).bind(input.measured_at).bind(input.overall_result.as_str())
            .fetch_one(&mut **tx).await.map_err(|e|format!("Lưu measurement: {e}"))?;
        Ok(id)
    }
    pub async fn find_by_id(pool: &PgPool, id: i64) -> Result<Option<Measurement>, String> {
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        let summary:Option<MeasurementSummary>=sqlx::query_as("SELECT m.*,t.result AS tip_result,b.result AS butt_result,t.load AS tip_load,b.load AS butt_load FROM measurements m LEFT JOIN measurement_stations t ON t.measurement_id=m.id AND t.station_type='TIP' LEFT JOIN measurement_stations b ON b.measurement_id=m.id AND b.station_type='BUTT' WHERE m.id=$1")
            .bind(id).fetch_optional(&mut *tx).await.map_err(|e|e.to_string())?;
        let measurement = match summary {
            Some(summary) => Some(Measurement {
                summary,
                stations: super::StationRepository::load(&mut tx, id).await?,
            }),
            None => None,
        };
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(measurement)
    }
    pub async fn list(
        pool: &PgPool,
        request: &MeasurementListRequest,
    ) -> Result<MeasurementListResponse, String> {
        let page = request.page.unwrap_or(1).max(1);
        let page_size = request.page_size.unwrap_or(20).clamp(1, 100);
        let offset = (page - 1)
            .checked_mul(page_size)
            .ok_or("Page vượt phạm vi")?;
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        let mut count = QueryBuilder::<Postgres>::new("SELECT count(*)");
        count.push(JOINS);
        filter(&mut count, request);
        let (total,): (i64,) = count
            .build_query_as()
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        let mut data = QueryBuilder::<Postgres>::new(
            "SELECT m.*,t.result AS tip_result,b.result AS butt_result,t.load AS tip_load,b.load AS butt_load",
        );
        data.push(JOINS);
        filter(&mut data, request);
        data.push(" ORDER BY m.measured_at DESC,m.id DESC LIMIT ")
            .push_bind(page_size)
            .push(" OFFSET ")
            .push_bind(offset);
        let items = data
            .build_query_as()
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(MeasurementListResponse {
            items,
            page,
            page_size,
            total,
        })
    }
    /// Xuất cùng một snapshot, bốn query theo lô cho toàn bộ cây; không N+1.
    pub async fn report(
        pool: &PgPool,
        request: &MeasurementListRequest,
    ) -> Result<Vec<Measurement>, String> {
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        let mut query = QueryBuilder::<Postgres>::new(
            "SELECT m.*,t.result AS tip_result,b.result AS butt_result,t.load AS tip_load,b.load AS butt_load",
        );
        query.push(JOINS);
        filter(&mut query, request);
        query.push(" ORDER BY m.measured_at DESC,m.id DESC");
        let summaries: Vec<MeasurementSummary> = query
            .build_query_as()
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        let ids: Vec<i64> = summaries.iter().map(|m| m.id).collect();
        let mut stations:Vec<MeasurementStation> = sqlx::query_as("SELECT * FROM measurement_stations WHERE measurement_id=ANY($1) ORDER BY measurement_id,station_type").bind(&ids).fetch_all(&mut *tx).await.map_err(|e|e.to_string())?;
        let station_ids: Vec<i64> = stations.iter().map(|s| s.id).collect();
        let mut points:Vec<MeasurementPoint> = sqlx::query_as("SELECT * FROM measurement_points WHERE station_id=ANY($1) ORDER BY station_id,point_number").bind(&station_ids).fetch_all(&mut *tx).await.map_err(|e|e.to_string())?;
        let point_ids: Vec<i64> = points.iter().map(|p| p.id).collect();
        let attempts:Vec<MeasurementAttempt> = sqlx::query_as("SELECT * FROM measurement_attempts WHERE point_id=ANY($1) ORDER BY point_id,attempt_number").bind(&point_ids).fetch_all(&mut *tx).await.map_err(|e|e.to_string())?;
        let mut by_point = std::collections::HashMap::<i64, Vec<MeasurementAttempt>>::new();
        for attempt in attempts {
            by_point.entry(attempt.point_id).or_default().push(attempt);
        }
        let mut by_station = std::collections::HashMap::<i64, Vec<MeasurementPoint>>::new();
        for mut point in points.drain(..) {
            point.attempts = by_point.remove(&point.id).unwrap_or_default();
            by_station.entry(point.station_id).or_default().push(point);
        }
        let mut by_measurement = std::collections::HashMap::<i64, Vec<MeasurementStation>>::new();
        for mut station in stations.drain(..) {
            station.points = by_station.remove(&station.id).unwrap_or_default();
            by_measurement
                .entry(station.measurement_id)
                .or_default()
                .push(station);
        }
        let result = summaries
            .into_iter()
            .map(|summary| Measurement {
                stations: by_measurement.remove(&summary.id).unwrap_or_default(),
                summary,
            })
            .collect();
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(result)
    }
    pub async fn statistics(
        pool: &PgPool,
        request: &MeasurementListRequest,
    ) -> Result<MeasurementStatistics, String> {
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        let mut q = QueryBuilder::<Postgres>::new(
            "SELECT count(*) AS total,count(*) FILTER(WHERE m.overall_result='OK') AS ok,count(*) FILTER(WHERE m.overall_result='NG') AS ng,count(*) FILTER(WHERE t.result='OK') AS tip_ok,count(*) FILTER(WHERE t.result='NG') AS tip_ng,count(*) FILTER(WHERE b.result='OK') AS butt_ok,count(*) FILTER(WHERE b.result='NG') AS butt_ng",
        );
        q.push(JOINS);
        filter(&mut q, request);
        let row = q
            .build()
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        let mut daily = QueryBuilder::<Postgres>::new(
            "SELECT (m.measured_at AT TIME ZONE 'Asia/Ho_Chi_Minh')::date::text AS date,count(*) AS total,count(*) FILTER(WHERE m.overall_result='OK') AS ok,count(*) FILTER(WHERE m.overall_result='NG') AS ng",
        );
        daily.push(JOINS);
        filter(&mut daily, request);
        daily.push(" GROUP BY (m.measured_at AT TIME ZONE 'Asia/Ho_Chi_Minh')::date ORDER BY (m.measured_at AT TIME ZONE 'Asia/Ho_Chi_Minh')::date");
        let daily = daily
            .build_query_as()
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        let mut distribution =
            QueryBuilder::<Postgres>::new("SELECT m.overall_result AS result,count(*) AS count");
        distribution.push(JOINS);
        filter(&mut distribution, request);
        distribution.push(" GROUP BY m.overall_result ORDER BY m.overall_result");
        let overall_distribution = distribution
            .build_query_as()
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        let total: i64 = row.try_get("total").map_err(|e| e.to_string())?;
        let ok: i64 = row.try_get("ok").map_err(|e| e.to_string())?;
        let ng: i64 = row.try_get("ng").map_err(|e| e.to_string())?;
        let tip_ng: i64 = row.try_get("tip_ng").map_err(|e| e.to_string())?;
        let butt_ng: i64 = row.try_get("butt_ng").map_err(|e| e.to_string())?;
        let station_results = vec![
            StationMeasurementStatistics {
                station: "Tip".into(),
                ok: row.try_get("tip_ok").map_err(|e| e.to_string())?,
                ng: tip_ng,
            },
            StationMeasurementStatistics {
                station: "Butt".into(),
                ok: row.try_get("butt_ok").map_err(|e| e.to_string())?,
                ng: butt_ng,
            },
        ];
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(MeasurementStatistics {
            total_measurements: total,
            total_ok: ok,
            total_ng: ng,
            pass_rate: if total == 0 {
                0.0
            } else {
                100.0 * ok as f64 / total as f64
            },
            tip_ng,
            butt_ng,
            daily,
            station_results,
            overall_distribution,
        })
    }
}
