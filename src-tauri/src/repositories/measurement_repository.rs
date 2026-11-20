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
    #[ignore = "Explicit Neon integration opt-in; TEMP fixtures on a verified connection"]
    async fn point_summary_merges_skus_from_decision_samples() {
        let pool=crate::database::test_support::pool(1).await;
        // Session-local fixtures shadow table names; never modify persistent tables.
        sqlx::raw_sql(
            r#"
            CREATE TEMP TABLE measurements(id bigint,product_code text,club_number int,
                measured_at timestamptz,overall_result text,source_id bigint,quality_error text);
            CREATE TEMP TABLE measurement_stations(id bigint,measurement_id bigint,
                station_type text,result text,load numeric);
            CREATE TEMP TABLE measurement_points(id bigint,station_id bigint,point_number int,
                position numeric,result text,attempt_count int,decide_attempt int);
            CREATE TEMP TABLE measurement_attempts(point_id bigint,attempt_number int,
                push_dist numeric,bend numeric);
            INSERT INTO measurements(id,product_code,club_number,measured_at,overall_result) SELECT n,CASE WHEN n<=100 THEN 'A' ELSE 'B' END,n,
                '2026-11-16 10:00:00+07','NG' FROM generate_series(1,124) n;
            INSERT INTO measurement_stations SELECT id,id,'BUTT','NG',
                CASE WHEN id<=100 THEN 10 ELSE 20 END FROM measurements;
            INSERT INTO measurement_stations SELECT id+1000,id,'TIP','OK',0 FROM measurements;
            INSERT INTO measurement_points SELECT id,id,
                CASE WHEN id=122 THEN 2 ELSE 1 END,
                CASE WHEN id=121 THEN 150 WHEN id=122 THEN NULL ELSE 50 END,
                CASE WHEN id>=123 THEN 'UNMEASURED' WHEN id=1 THEN 'NG' ELSE 'OK' END,
                CASE WHEN id>=123 THEN 0 WHEN id=1 THEN 4 ELSE 1 END,
                CASE WHEN id>=123 THEN NULL WHEN id=1 THEN 4 ELSE 1 END FROM measurements;
            INSERT INTO measurement_attempts SELECT id,CASE WHEN id=1 THEN 4 ELSE 1 END,
                CASE WHEN id=122 THEN NULL WHEN id<=100 THEN 0 ELSE 24 END,
                CASE WHEN id=122 THEN NULL WHEN id<=100 THEN 0 WHEN id=121 THEN 30 ELSE 12 END
                FROM measurements WHERE id<=122;
            INSERT INTO measurement_attempts SELECT 1,n,999,999 FROM generate_series(1,3) n;
            INSERT INTO measurement_points VALUES(1001,1001,1,50,'OK',1,1);
            INSERT INTO measurement_attempts VALUES(1001,1,0,5);
        "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let request = MeasurementListRequest {
            page: Some(99),
            page_size: Some(1),
            ..Default::default()
        };
        let all = MeasurementRepository::quality_statistics(&pool, &request)
            .await
            .unwrap();
        assert_eq!(all.point_summary.len(), 4); // Separate positions/stations; omit unmeasured points.
        let merged = all
            .point_summary
            .iter()
            .find(|g| g.station == "BUTT" && g.position.as_deref() == Some("50"))
            .unwrap();
        assert_eq!(merged.station, "BUTT");
        assert_eq!(merged.point_number, 1);
        let decimal = |value: &Option<String>| {
            value
                .as_ref()
                .unwrap()
                .parse::<bigdecimal::BigDecimal>()
                .unwrap()
        };
        assert_eq!(decimal(&merged.bend_avg), bigdecimal::BigDecimal::from(2)); // 100*0 + 20*12, divided by 120.
        assert_eq!(
            decimal(&merged.push_dist_avg),
            bigdecimal::BigDecimal::from(4)
        );
        assert_eq!(decimal(&merged.bend_min), bigdecimal::BigDecimal::from(0));
        assert_eq!(decimal(&merged.bend_max), bigdecimal::BigDecimal::from(12));
        assert_eq!((merged.retested, merged.ng), (1, 1)); // Not four attempts.
        let load = decimal(&merged.load_avg);
        assert!(
            load > "11.66".parse::<bigdecimal::BigDecimal>().unwrap()
                && load < "11.67".parse::<bigdecimal::BigDecimal>().unwrap()
        );
        let missing = all
            .point_summary
            .iter()
            .find(|g| g.point_number == 2)
            .unwrap();
        assert!(
            missing.position.is_none()
                && missing.bend_avg.is_none()
                && missing.push_dist_avg.is_none()
        );
        assert_eq!(decimal(&missing.load_avg), bigdecimal::BigDecimal::from(20));
        assert_eq!((missing.retested, missing.ng), (0, 0));
        assert_eq!(
            all.points
                .iter()
                .filter(|g| g.station == "BUTT" && g.position.as_deref() == Some("50"))
                .count(),
            2
        );
        let a = MeasurementRepository::quality_statistics(
            &pool,
            &MeasurementListRequest {
                product_code: Some("A".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(a.point_summary.len(), 2);
        assert_eq!(
            decimal(&a.point_summary[0].bend_avg),
            bigdecimal::BigDecimal::from(0)
        );
        let single_request = MeasurementListRequest {
            product_code: Some("B".into()),
            club_number: Some(121),
            measured_from: Some(
                chrono::DateTime::parse_from_rfc3339("2026-11-16T00:00:00+07:00")
                    .unwrap()
                    .to_utc(),
            ),
            measured_to: Some(
                chrono::DateTime::parse_from_rfc3339("2026-11-17T00:00:00+07:00")
                    .unwrap()
                    .to_utc(),
            ),
            result: Some(crate::enums::MeasurementResult::Ng),
            butt_result: Some(crate::enums::MeasurementResult::Ng),
            tip_result: Some(crate::enums::MeasurementResult::Ok),
            ..Default::default()
        };
        let single = MeasurementRepository::quality_statistics(&pool, &single_request)
            .await
            .unwrap();
        assert_eq!(single.point_summary.len(), 1);
        assert_eq!(single.point_summary[0].position.as_deref(), Some("150"));
        assert_eq!(
            decimal(&single.point_summary[0].bend_avg),
            bigdecimal::BigDecimal::from(30)
        );
        let empty = MeasurementRepository::quality_statistics(
            &pool,
            &MeasurementListRequest {
                tip_result: Some(crate::enums::MeasurementResult::Ng),
                ..single_request
            },
        )
        .await
        .unwrap();
        assert!(empty.point_summary.is_empty());
        assert!(empty.butt_histogram.is_empty());

        // New connection for the second TEMP fixture; no DROP/TRUNCATE/DELETE.
        pool.close().await;
        let pool=crate::database::test_support::pool(1).await;
        sqlx::raw_sql(r#"
            CREATE TEMP TABLE measurements(id bigint,product_code text,club_number int,
                measured_at timestamptz,overall_result text,source_id bigint,quality_error text);
            CREATE TEMP TABLE measurement_stations(id bigint,measurement_id bigint,
                station_type text,result text,load numeric);
            CREATE TEMP TABLE measurement_points(id bigint,station_id bigint,point_number int,
                position numeric,result text,attempt_count int,decide_attempt int);
            CREATE TEMP TABLE measurement_attempts(point_id bigint,attempt_number int,
                push_dist numeric,bend numeric);
            INSERT INTO measurements(id,product_code,club_number,measured_at,overall_result) VALUES
                (1,'A',1,'2026-11-16 10:00:00+07','NG'),
                (2,'B',2,'2026-11-16 11:00:00+07','NG'),
                (3,'A',3,'2026-11-17 10:00:00+07','OK'),
                (4,'A',4,'2026-11-16 12:00:00+07','OK');
            INSERT INTO measurement_stations SELECT id,id,'BUTT',overall_result,10 FROM measurements;
            INSERT INTO measurement_stations SELECT id+100,id,'TIP','OK',20 FROM measurements;
            INSERT INTO measurement_points VALUES
                (11,1,1,50,'NG',2,2),(12,1,2,75,'OK',1,1),
                (13,1,3,100,'OK',1,1),(14,1,4,125,'NG',1,1),
                (21,2,1,50,'NG',1,1),(22,2,2,NULL,'UNMEASURED',0,NULL),
                (23,2,3,100,'OK',1,1),(24,2,4,125,'SKIPPED',1,1),
                (31,3,1,50,'OK',1,1),(41,4,1,50,'OK',1,1),
                (101,101,1,50,'OK',1,1),(102,102,1,50,'OK',1,1);
            INSERT INTO measurement_attempts VALUES
                (11,1,999,999),(11,2,999,-1),(12,1,999,0),(13,1,999,1),
                (14,1,999,2),(21,1,999,-2),(23,1,999,NULL),(24,1,999,100),
                (31,1,999,8),(41,1,999,NULL),(101,1,999,99),(102,1,999,99);
        "#).execute(&pool).await.unwrap();
        let day_request = MeasurementListRequest {
            measured_from: single_request.measured_from,
            measured_to: single_request.measured_to,
            ..Default::default()
        };
        let day = MeasurementRepository::quality_statistics(&pool, &day_request)
            .await
            .unwrap();
        assert_eq!(day.butt_histogram.iter().map(|b| b.count).sum::<i64>(), 5);
        assert_eq!(day.butt_histogram[5].count, 1); // Zero is a real decision Bend.
        assert_eq!(day.retested_clubs, 1); // Two attempts still contribute one decision sample.
        for (actual, expected) in [
            (&day.butt_histogram[0].lower, -2),
            (&day.butt_histogram[9].upper, 2),
        ] {
            assert_eq!(
                actual.parse::<bigdecimal::BigDecimal>().unwrap(),
                bigdecimal::BigDecimal::from(expected)
            );
        }
        let four = MeasurementRepository::quality_statistics(
            &pool,
            &MeasurementListRequest {
                product_code: Some("A".into()),
                club_number: Some(1),
                result: Some(crate::enums::MeasurementResult::Ng),
                butt_result: Some(crate::enums::MeasurementResult::Ng),
                tip_result: Some(crate::enums::MeasurementResult::Ok),
                ..day_request.clone()
            },
        )
        .await
        .unwrap();
        assert_eq!(four.butt_histogram.iter().map(|b| b.count).sum::<i64>(), 4); // Includes P4 NG.
        let one = MeasurementRepository::quality_statistics(
            &pool,
            &MeasurementListRequest {
                product_code: Some("B".into()),
                ..day_request.clone()
            },
        )
        .await
        .unwrap();
        assert_eq!(one.butt_histogram.len(), 1);
        assert_eq!(one.butt_histogram[0].count, 1);
        assert_eq!(one.butt_histogram[0].lower, one.butt_histogram[0].upper);
        let null_only = MeasurementRepository::quality_statistics(
            &pool,
            &MeasurementListRequest {
                club_number: Some(4),
                ..day_request
            },
        )
        .await
        .unwrap();
        assert!(null_only.butt_histogram.is_empty());
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "Explicit Neon integration opt-in; owned fixture then read-only report queries"]
    async fn postgres_report_filters_pagination_and_nested_details_read_only() {
        let write_pool=crate::database::test_support::pool(2).await;
        let key=crate::database::test_support::source_key("report");
        let source=super::super::SourceRepository::upsert(&write_pool,&key,None).await.unwrap();
        let mut row=crate::database::test_support::receipt(1);row.source_id=source.id;
        // Use an isolated ASCII ProductCode in raw, rather than a QC fallback.
        let product = format!("IT-{}", source.id);
        assert!(product.len() <= 20);
        for word in 4..14 { row.raw_registers[word] = serde_json::json!(0); }
        for (index, bytes) in product.as_bytes().chunks(2).enumerate() {
            row.raw_registers[4 + index] = serde_json::json!(u16::from_le_bytes([bytes[0], *bytes.get(1).unwrap_or(&0)]));
        }
        row.publication_session=Some(crate::database::test_support::session());row.assigned_sku=Some(crate::database::test_support::sku());
        let draft=crate::services::inbox_service::draft_from_inbox(&row).unwrap();
        let mut tx=write_pool.begin().await.unwrap();crate::services::MeasurementService::insert(&mut tx,&draft).await.unwrap();tx.commit().await.unwrap();write_pool.close().await;
        let pool=crate::database::test_support::readonly_pool(2).await;
        let scope=MeasurementListRequest{product_code:Some(product),..Default::default()};
        let all = MeasurementRepository::report(&pool, &scope)
            .await
            .unwrap();
        let quality =
            MeasurementRepository::quality_statistics(&pool, &scope)
                .await
                .unwrap();
        let is_measured = |point: &MeasurementPoint| {
            point.attempt_count > 0
                && matches!(
                    point.result,
                    crate::enums::MeasurementResult::Ok | crate::enums::MeasurementResult::Ng
                )
        };
        let retested = all
            .iter()
            .filter(|m| {
                m.stations.iter().any(|s| {
                    s.points
                        .iter()
                        .any(|p| is_measured(p) && p.attempt_count > 1)
                })
            })
            .count();
        assert_eq!(quality.retested_clubs as usize, retested);
        for group in &quality.points {
            let matching: Vec<_> = all
                .iter()
                .filter(|m| m.summary.product_code == group.product_code)
                .flat_map(|m| m.stations.iter())
                .filter(|s| s.station_type.as_str() == group.station)
                .flat_map(|s| s.points.iter())
                .filter(|p| {
                    is_measured(p)
                        && p.point_number == group.point_number
                        && p.position
                            == group
                                .position
                                .as_ref()
                                .map(|s| s.parse::<bigdecimal::BigDecimal>().unwrap())
                })
                .collect();
            assert_eq!(group.measured as usize, matching.len());
            assert_eq!(
                group.ng as usize,
                matching
                    .iter()
                    .filter(|p| p.result == crate::enums::MeasurementResult::Ng)
                    .count()
            );
            assert_eq!(
                group.retested as usize,
                matching.iter().filter(|p| p.attempt_count > 1).count()
            );
            let bends: Vec<_> = matching
                .iter()
                .filter_map(|p| {
                    p.attempts
                        .iter()
                        .find(|a| {
                            Some(a.attempt_number) == p.decide_attempt
                                && a.attempt_number <= p.attempt_count
                        })
                        .and_then(|a| a.bend.as_ref())
                })
                .collect();
            assert_eq!(group.bend_count as usize, bends.len());
            assert_eq!(
                group.histogram.iter().map(|bin| bin.count).sum::<i64>(),
                group.bend_count
            );
            let minimum = group
                .bend_min
                .as_ref()
                .map(|s| s.parse::<bigdecimal::BigDecimal>().unwrap());
            let maximum = group
                .bend_max
                .as_ref()
                .map(|s| s.parse::<bigdecimal::BigDecimal>().unwrap());
            assert_eq!(minimum.as_ref(), bends.iter().copied().min());
            assert_eq!(maximum.as_ref(), bends.iter().copied().max());
        }
        let boundary: String = sqlx::query_scalar("SELECT (TIMESTAMPTZ '2026-11-09 17:30:00+00' AT TIME ZONE 'Asia/Ho_Chi_Minh')::date::text")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(boundary, "2026-11-10");
        let included:Vec<bool> = sqlx::query_scalar("SELECT instant >= TIMESTAMPTZ '2026-11-10 00:00:00+07' AND instant < TIMESTAMPTZ '2026-11-11 00:00:00+07' FROM (VALUES (TIMESTAMPTZ '2026-11-10 23:59:59.999000+07'),(TIMESTAMPTZ '2026-11-10 23:59:59.999500+07'),(TIMESTAMPTZ '2026-11-11 00:00:00.000000+07')) AS samples(instant)")
            .fetch_all(&pool).await.unwrap();
        assert_eq!(included, [true, true, false]);
        let multi:bool=sqlx::query_scalar("SELECT TIMESTAMPTZ '2026-11-12 23:59:59.999500+07' >= TIMESTAMPTZ '2026-11-10 00:00:00+07' AND TIMESTAMPTZ '2026-11-12 23:59:59.999500+07' < TIMESTAMPTZ '2026-11-13 00:00:00+07'")
            .fetch_one(&pool).await.unwrap();
        assert!(multi);

        let daily = MeasurementRepository::statistics(&pool, &scope)
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
                assert_eq!(station.points.len(), if station.station_type==crate::models::StationType::Tip {1} else {4});
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
            let summary = MeasurementRepository::sku_summary(&pool, &request)
                .await
                .unwrap();
            assert_eq!(summary.total, 1);
            assert_eq!(summary.items[0].total as usize, filtered.len());
            assert_eq!(summary.items[0].product_code, first.summary.product_code);
            assert_eq!(
                summary.items[0].first_measured_at,
                first.summary.measured_at
            );
            assert_eq!(summary.items[0].last_measured_at, first.summary.measured_at);
            assert_eq!(summary.items[0].ok, stats.total_ok);
            assert_eq!(summary.items[0].ng, stats.total_ng);
            assert_eq!(summary.items[0].butt_ng, stats.butt_ng);
            assert_eq!(summary.items[0].tip_ng, stats.tip_ng);
            let history = MeasurementRepository::history_page(&pool, &request)
                .await
                .unwrap();
            assert_eq!(history.total as usize, filtered.len());
            assert_eq!(history.items.len(), 1); // page_size=1, never load every measurement.
            let actual = &history.items[0];
            let full = filtered
                .iter()
                .find(|m| m.summary.id == actual.summary.id)
                .unwrap();
            for value in &actual.decision_points {
                let station = full
                    .stations
                    .iter()
                    .find(|s| s.station_type.as_str() == value.station)
                    .unwrap();
                let point = station
                    .points
                    .iter()
                    .find(|p| p.point_number == value.point_number)
                    .unwrap();
                let expected = if point.attempt_count > 0
                    && point.result != crate::enums::MeasurementResult::Unmeasured
                {
                    point
                        .attempts
                        .iter()
                        .find(|a| Some(a.attempt_number) == point.decide_attempt)
                        .and_then(|a| a.bend.clone())
                } else {
                    None
                };
                assert_eq!(value.bend, expected);
            }
        }
        let expected_skus: std::collections::BTreeSet<_> =
            all.iter().map(|m| m.summary.product_code.clone()).collect();
        let mut actual_skus = std::collections::BTreeSet::new();
        let mut page = 1;
        loop {
            let options = MeasurementRepository::sku_options(&pool, "", page)
                .await
                .unwrap();
            assert!(options.items.len() <= 30);
            actual_skus.extend(options.items);
            if !options.has_more {
                break;
            }
            page += 1;
        }
        assert_eq!(actual_skus, expected_skus);
        let empty = MeasurementListRequest {
            measured_from: Some(chrono::DateTime::from_timestamp(0, 0).unwrap()),
            measured_to: Some(chrono::DateTime::from_timestamp(0, 0).unwrap()),
            ..Default::default()
        };
        assert_eq!(
            MeasurementRepository::sku_summary(&pool, &empty)
                .await
                .unwrap()
                .total,
            0
        );
        assert!(
            MeasurementRepository::history_page(&pool, &empty)
                .await
                .unwrap()
                .items
                .is_empty()
        );
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
    pub async fn sku_options(
        pool: &PgPool,
        search: &str,
        page: i64,
    ) -> Result<MeasurementSkuOptions, String> {
        let offset = (page.max(1) - 1)
            .checked_mul(30)
            .ok_or("Page vượt phạm vi")?;
        let mut q = QueryBuilder::<Postgres>::new(
            "SELECT DISTINCT product_code FROM measurements WHERE 1=1",
        );
        if !search.is_empty() {
            // Literal substring search: '%' and '_' in a real SKU are not wildcards.
            q.push(" AND strpos(lower(product_code),lower(")
                .push_bind(search)
                .push("))>0");
        }
        q.push(" ORDER BY product_code LIMIT 31 OFFSET ")
            .push_bind(offset);
        let rows: Vec<(String,)> = q
            .build_query_as()
            .fetch_all(pool)
            .await
            .map_err(|e| e.to_string())?;
        let has_more = rows.len() > 30;
        Ok(MeasurementSkuOptions {
            items: rows.into_iter().take(30).map(|r| r.0).collect(),
            has_more,
        })
    }

    pub async fn sku_summary(
        pool: &PgPool,
        request: &MeasurementListRequest,
    ) -> Result<MeasurementSkuPage, String> {
        let page = request.page.unwrap_or(1).max(1);
        let page_size = request.page_size.unwrap_or(10).clamp(1, 100);
        let offset = (page - 1)
            .checked_mul(page_size)
            .ok_or("Page vượt phạm vi")?;
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        let mut count = QueryBuilder::<Postgres>::new("SELECT count(DISTINCT m.product_code)");
        count.push(JOINS);
        filter(&mut count, request);
        let (total,): (i64,) = count
            .build_query_as()
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        let mut q = QueryBuilder::<Postgres>::new(
            "SELECT m.product_code,min(m.measured_at) AS first_measured_at,max(m.measured_at) AS last_measured_at,count(*) AS total,count(*) FILTER(WHERE m.overall_result='OK') AS ok,count(*) FILTER(WHERE m.overall_result='NG') AS ng,count(*) FILTER(WHERE b.result='NG' AND m.overall_result IN ('OK','NG')) AS butt_ng,count(*) FILTER(WHERE t.result='NG' AND m.overall_result IN ('OK','NG')) AS tip_ng",
        );
        q.push(JOINS);
        filter(&mut q, request);
        q.push(" GROUP BY m.product_code ORDER BY max(m.measured_at) DESC,m.product_code LIMIT ")
            .push_bind(page_size)
            .push(" OFFSET ")
            .push_bind(offset);
        let items = q
            .build_query_as()
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(MeasurementSkuPage {
            items,
            total,
            page,
            page_size,
        })
    }

    pub async fn history_page(
        pool: &PgPool,
        request: &MeasurementListRequest,
    ) -> Result<MeasurementHistoryPage, String> {
        let list = Self::list(pool, request).await?;
        let ids: Vec<_> = list.items.iter().map(|m| m.id).collect();
        // One bounded query for decision values on this page, not one request per club.
        let points: Vec<HistoryDecisionPoint> = sqlx::query_as("SELECT s.measurement_id,s.station_type AS station,p.point_number,a.bend AS bend FROM measurement_stations s JOIN measurement_points p ON p.station_id=s.id LEFT JOIN measurement_attempts a ON a.point_id=p.id AND a.attempt_number=p.decide_attempt AND p.decide_attempt BETWEEN 1 AND 4 WHERE s.measurement_id=ANY($1) AND ((s.station_type='TIP' AND p.point_number=1) OR (s.station_type='BUTT' AND p.point_number BETWEEN 1 AND 4)) ORDER BY s.measurement_id,s.station_type,p.point_number")
            .bind(&ids).fetch_all(pool).await.map_err(|e| e.to_string())?;
        let mut grouped = std::collections::HashMap::<i64, Vec<HistoryDecisionPoint>>::new();
        for point in points {
            grouped.entry(point.measurement_id).or_default().push(point);
        }
        let items = list
            .items
            .into_iter()
            .map(|summary| {
                let decision_points = grouped.remove(&summary.id).unwrap_or_default();
                MeasurementHistoryItem {
                    summary,
                    decision_points,
                }
            })
            .collect();
        Ok(MeasurementHistoryPage {
            items,
            total: list.total,
            page: list.page,
            page_size: list.page_size,
        })
    }

    /// Aggregate only: one decision value per measurement/point, never all attempts
    /// sent to the UI. A single SELECT provides a consistent read-only snapshot.
    pub async fn quality_statistics(
        pool: &PgPool,
        request: &MeasurementListRequest,
    ) -> Result<MeasurementQualityStatistics, String> {
        let mut q = QueryBuilder::<Postgres>::new(
            "WITH filtered AS (SELECT m.id,m.product_code,m.measured_at,m.overall_result,(m.product_code='DEMO-AS200' AND EXISTS(SELECT 1 FROM plc_sources ps WHERE ps.id=m.source_id AND ps.source_key='golf-report-simulator')) AS simulator_demo",
        );
        q.push(JOINS);
        filter(&mut q, request);
        q.push(" AND m.overall_result IN ('OK','NG')");
        q.push(r#"), half_hour AS (
            SELECT to_timestamp(floor(extract(epoch FROM measured_at)/1800)*1800) AS at,
                count(*) FILTER(WHERE overall_result='OK') AS ok,
                count(*) FILTER(WHERE overall_result='NG') AS ng
            FROM filtered GROUP BY 1
        ), demo_scope AS (
            SELECT count(*)>0 AND coalesce(bool_and(simulator_demo),false) AS valid FROM filtered
        ), samples AS (
            SELECT m.id AS measurement_id,m.product_code,s.station_type AS station,
                p.point_number,p.position,p.result,p.attempt_count,s.load,a.push_dist,a.bend
            FROM filtered m
            JOIN measurement_stations s ON s.measurement_id=m.id
            JOIN measurement_points p ON p.station_id=s.id
            LEFT JOIN measurement_attempts a ON a.point_id=p.id
                AND a.attempt_number=p.decide_attempt
                AND p.decide_attempt BETWEEN 1 AND 4
            WHERE p.result IN ('OK','NG')
                AND ((s.station_type='TIP' AND p.point_number=1)
                    OR (s.station_type='BUTT' AND p.point_number BETWEEN 1 AND 4))
        ), grouped AS (
            SELECT product_code,station,point_number,position,count(*) AS measured,
                count(DISTINCT measurement_id) FILTER(WHERE result='NG') AS ng,
                count(*) FILTER(WHERE attempt_count>1) AS retested,
                avg(load)::text AS load_avg,avg(push_dist)::text AS push_dist_avg,
                count(bend) AS bend_count,min(bend) AS bend_min,avg(bend) AS bend_avg,
                max(bend) AS bend_max,stddev_samp(bend) AS bend_stddev
            FROM samples GROUP BY product_code,station,point_number,position
        ), point_summary AS (
            SELECT station,point_number,position,
                avg(load)::text AS load_avg,avg(push_dist)::text AS push_dist_avg,
                min(bend)::text AS bend_min,avg(bend)::text AS bend_avg,
                max(bend)::text AS bend_max,
                count(DISTINCT measurement_id) FILTER(WHERE attempt_count>1) AS retested,
                count(DISTINCT measurement_id) FILTER(WHERE result='NG') AS ng
            FROM samples GROUP BY station,point_number,position
        ), binned AS (
            SELECT s.product_code,s.station,s.point_number,s.position,
                CASE WHEN g.bend_max=g.bend_min THEN 0 ELSE
                    least(9,greatest(0,floor((s.bend-g.bend_min)*10 /
                        nullif(g.bend_max-g.bend_min,0))::int)) END AS bin,
                count(*) AS amount
            FROM samples s JOIN grouped g USING(product_code,station,point_number)
            WHERE s.position IS NOT DISTINCT FROM g.position AND s.bend IS NOT NULL
            GROUP BY s.product_code,s.station,s.point_number,s.position,bin
        ), butt_samples AS (
            SELECT bend FROM samples
            WHERE station='BUTT' AND point_number BETWEEN 1 AND 4 AND bend IS NOT NULL
        ), butt_range AS (
            SELECT count(*) AS amount,min(bend) AS lo,max(bend) AS hi,
                avg(bend)::text AS mean,stddev_samp(bend)::text AS stddev FROM butt_samples
        ), butt_bins AS (
            SELECT CASE WHEN r.hi=r.lo THEN 0 ELSE
                least(9,greatest(0,floor((s.bend-r.lo)*10/nullif(r.hi-r.lo,0))::int)) END AS bin,
                count(*) AS amount
            FROM butt_samples s CROSS JOIN butt_range r GROUP BY bin
        ), demo_binned AS (
            SELECT CASE WHEN r.hi=r.lo THEN 0 ELSE
                least(9,greatest(0,floor((s.bend-r.lo)*10/nullif(r.hi-r.lo,0))::int)) END AS bin,
                CASE WHEN s.bend<0 THEN 'BELOW' WHEN s.bend>2 THEN 'ABOVE' ELSE 'WITHIN' END AS zone,
                count(*) AS amount
            FROM butt_samples s CROSS JOIN butt_range r
            WHERE (SELECT valid FROM demo_scope) GROUP BY bin,zone
        ), demo_intervals AS (
            SELECT b.*,r.hi,
                CASE WHEN r.lo=r.hi THEN r.lo ELSE r.lo+(r.hi-r.lo)*b.bin/10 END AS bin_lo,
                CASE WHEN r.lo=r.hi THEN r.hi ELSE r.lo+(r.hi-r.lo)*(b.bin+1)/10 END AS bin_hi
            FROM demo_binned b CROSS JOIN butt_range r
        ) SELECT jsonb_build_object(
            'halfHour',coalesce((SELECT jsonb_agg(jsonb_build_object('at',at,'ok',ok,'ng',ng) ORDER BY at) FROM half_hour),'[]'::jsonb),
            'demoLimits',CASE WHEN (SELECT valid FROM demo_scope) THEN jsonb_build_object('lower','0','upper','2') ELSE NULL END,
            'demoHistogram',coalesce((SELECT jsonb_agg(jsonb_build_object(
                'lower',(CASE WHEN zone='BELOW' THEN bin_lo WHEN zone='WITHIN' THEN greatest(bin_lo,0) ELSE greatest(bin_lo,2) END)::text,
                'upper',(CASE WHEN zone='ABOVE' THEN bin_hi WHEN zone='WITHIN' THEN least(bin_hi,2) ELSE least(bin_hi,0) END)::text,
                'count',amount,'zone',zone,
                'lowerInclusive',zone<>'ABOVE' OR bin_lo>2,
                'upperInclusive',(zone='WITHIN' AND bin_hi>=2) OR (bin_hi=hi AND NOT(zone='BELOW' AND bin_hi>=0))
                ) ORDER BY bin,CASE zone WHEN 'BELOW' THEN 0 WHEN 'WITHIN' THEN 1 ELSE 2 END) FROM demo_intervals),'[]'::jsonb),
            'buttSummary',(SELECT jsonb_build_object('count',amount,'mean',mean,'stddev',stddev) FROM butt_range),
            'retestedClubs',(SELECT count(DISTINCT measurement_id) FROM samples WHERE attempt_count>1),
            'buttHistogram',coalesce((SELECT jsonb_agg(jsonb_build_object(
                'lower',(CASE WHEN r.lo=r.hi THEN r.lo ELSE r.lo+(r.hi-r.lo)*bins.n/10 END)::text,
                'upper',(CASE WHEN r.lo=r.hi THEN r.hi ELSE r.lo+(r.hi-r.lo)*(bins.n+1)/10 END)::text,
                'count',coalesce(b.amount,0)) ORDER BY bins.n)
                FROM butt_range r CROSS JOIN LATERAL generate_series(0,CASE WHEN r.amount=0 THEN -1
                    WHEN r.lo=r.hi THEN 0 ELSE 9 END) bins(n)
                LEFT JOIN butt_bins b ON b.bin=bins.n),'[]'::jsonb),
            'pointSummary',coalesce((SELECT jsonb_agg(jsonb_build_object(
                'station',s.station,'pointNumber',s.point_number,'position',s.position::text,
                'loadAvg',s.load_avg,'pushDistAvg',s.push_dist_avg,
                'bendMin',s.bend_min,'bendAvg',s.bend_avg,'bendMax',s.bend_max,
                'retested',s.retested,'ng',s.ng
                ) ORDER BY s.station,s.point_number,s.position) FROM point_summary s),'[]'::jsonb),
            'points',coalesce((SELECT jsonb_agg(jsonb_build_object(
                'productCode',g.product_code,'station',g.station,'pointNumber',g.point_number,
                'position',g.position::text,'measured',g.measured,'ng',g.ng,'retested',g.retested,
                'loadAvg',g.load_avg,'pushDistAvg',g.push_dist_avg,'bendCount',g.bend_count,
                'bendMin',g.bend_min::text,'bendAvg',g.bend_avg::text,
                'bendMax',g.bend_max::text,'bendStddev',g.bend_stddev::text,
                'histogram',coalesce((SELECT jsonb_agg(jsonb_build_object(
                    'lower',(g.bend_min+(g.bend_max-g.bend_min)*bins.n/10)::text,
                    'upper',(CASE WHEN g.bend_min=g.bend_max THEN g.bend_max
                        ELSE g.bend_min+(g.bend_max-g.bend_min)*(bins.n+1)/10 END)::text,
                    'count',coalesce(b.amount,0)) ORDER BY bins.n)
                    FROM generate_series(0,CASE WHEN g.bend_count=0 THEN -1
                        WHEN g.bend_min=g.bend_max THEN 0 ELSE 9 END) bins(n)
                    LEFT JOIN binned b ON b.product_code=g.product_code AND b.station=g.station
                        AND b.point_number=g.point_number AND b.position IS NOT DISTINCT FROM g.position
                        AND b.bin=bins.n),'[]'::jsonb)
                ) ORDER BY g.product_code,g.station,g.point_number,g.position) FROM grouped g),'[]'::jsonb)
        )"#);
        let (value,): (serde_json::Value,) = q
            .build_query_as()
            .fetch_one(pool)
            .await
            .map_err(|e| format!("Không tải được thống kê chất lượng: {e}"))?;
        serde_json::from_value(value).map_err(|e| e.to_string())
    }

    pub async fn insert(
        tx: &mut Transaction<'_, Postgres>,
        input: &MeasurementDraft,
    ) -> Result<i64, String> {
        let (id,):(i64,)=sqlx::query_as("INSERT INTO measurements(source_id,session_id,record_seq,total_count,product_code,club_number,measured_at,overall_result,publication_count,quality_error,sku_origin) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) RETURNING id")
            .bind(input.source_id).bind(&input.confirmed_session_id).bind(input.record_seq).bind(input.total_count).bind(&input.product_code).bind(input.club_number).bind(input.measured_at).bind(input.overall_result.as_str()).bind(input.publication_count).bind(&input.quality_error).bind(&input.sku_origin)
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
                receipt: sqlx::query_as::<_, MeasurementReceipt>("SELECT i.id AS inbox_id,s.source_key,i.publication_session,i.publication_count,i.word_order FROM plc_inbox i JOIN plc_sources s ON s.id=i.source_id WHERE i.measurement_id=$1")
                    .bind(id).fetch_optional(&mut *tx).await.map_err(|e| e.to_string())?,
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
                receipt: None,
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
        q.push(" AND m.overall_result IN ('OK','NG')");
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
        daily.push(" AND m.overall_result IN ('OK','NG')");
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
        distribution.push(" AND m.overall_result IN ('OK','NG')");
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
