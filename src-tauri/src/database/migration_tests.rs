use super::*;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{
    borrow::Cow,
    str::FromStr,
    sync::atomic::{AtomicU64, Ordering},
};

fn history(retired: bool) -> Vec<MigrationHistory> {
    MIGRATOR
        .iter()
        .filter(|m| retired || m.version < REGISTRY_RETIREMENT_VERSION)
        .map(|m| MigrationHistory {
            version: m.version,
            description: m.description.to_string(),
            success: true,
        })
        .collect()
}
fn tables(registry: bool) -> Vec<String> {
    let mut tables: Vec<_> = [
        "_sqlx_migrations",
        "plc_sources",
        "plc_inbox",
        "plc_acquisition",
        "measurements",
        "measurement_stations",
        "measurement_points",
        "measurement_attempts",
        "measurement_settings",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    if registry {
        tables.push("plc_source_registry".into());
    }
    tables
}
#[test]
fn registry_required_until_retirement_and_forbidden_afterwards() {
    assert!(validate_history(&tables(true), &history(false)).is_ok());
    assert!(validate_history(&tables(false), &history(false)).is_err());
    assert!(validate_history(&tables(false), &history(true)).is_ok());
    assert!(validate_history(&tables(true), &history(true)).is_err());
}
#[test]
fn retirement_cannot_mask_missing_failed_duplicate_or_unknown_history() {
    let valid = history(true);
    for i in 0..valid.len() {
        let mut missing = history(true);
        missing.remove(i);
        if i + 1 < valid.len() {
            assert!(validate_history(&tables(false), &missing).is_err());
        }
        let mut failed = history(true);
        failed[i].success = false;
        assert!(validate_history(&tables(false), &failed).is_err());
    }
    let mut duplicate = history(true);
    duplicate.push(MigrationHistory {
        version: INITIAL_VERSION,
        description: "duplicate".into(),
        success: true,
    });
    assert!(validate_history(&tables(false), &duplicate).is_err());
    let mut unknown = history(true);
    unknown.push(MigrationHistory {
        version: REGISTRY_RETIREMENT_VERSION + 1,
        description: "unknown".into(),
        success: true,
    });
    assert!(validate_history(&tables(false), &unknown).is_err());
}

#[test]
fn applied_settings_migration_requires_settings_table() {
    let latest = history(true);
    let mut schema = tables(false);
    assert!(validate_history(&schema, &latest).is_ok());
    schema.retain(|table| table != "measurement_settings");
    assert!(
        validate_history(&schema, &latest)
            .unwrap_err()
            .contains("measurement_settings")
    );
    let pending: Vec<_> = latest
        .into_iter()
        .filter(|m| m.version < MEASUREMENT_SETTINGS_VERSION)
        .collect();
    assert!(validate_history(&schema, &pending).is_ok());
}

#[test]
fn historical_settings_migration_matches_verified_applied_checksum() {
    // Read-only Neon audit confirmed this SHA-384. Never rewrite SQLx history.
    let migration = MIGRATOR
        .iter()
        .find(|m| m.version == MEASUREMENT_SETTINGS_VERSION)
        .unwrap();
    let checksum = migration
        .checksum
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        checksum,
        "1508c54f177eab759fabd612d71ba5d7695e4755322c6b8955585cfefe4cef42fb77dabc530437c4a7ef5fad4677725a"
    );
}

// Separate local database ONLY. Never read the application's Neon config/helper.
pub(crate) async fn isolated() -> PgPool {
    let url = std::env::var("GOLF_MIGRATION_TEST_DATABASE_URL")
        .expect("Explicit local migration test URL required");
    let options = PgConnectOptions::from_str(&url).expect("Invalid local test URL");
    assert!(matches!(options.get_host(), "127.0.0.1" | "::1"));
    assert_eq!(options.get_database(), Some("golf_report_migration_test"));
    let root = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await
        .unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()::text")
        .fetch_one(&root)
        .await
        .unwrap();
    assert_eq!(database, "golf_report_migration_test");
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let schema = format!(
        "gr_mig_{}_{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    );
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&root)
        .await
        .unwrap();
    root.close().await;
    PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.options([
            ("search_path", schema.as_str()),
            ("statement_timeout", "15000"),
        ]))
        .await
        .unwrap()
}
async fn before_retirement(pool: &PgPool) {
    let old = sqlx::migrate::Migrator {
        migrations: Cow::Owned(
            MIGRATOR
                .iter()
                .filter(|m| m.version < REGISTRY_RETIREMENT_VERSION)
                .cloned()
                .collect(),
        ),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    old.run(pool).await.unwrap();
}
async fn assert_retired(pool: &PgPool) {
    let (actual_tables, applied) = audit(pool).await.unwrap();
    let mut expected = tables(false);
    expected.sort();
    assert_eq!(actual_tables, expected);
    assert_eq!(applied.len(), MIGRATOR.iter().count());
    assert!(validate_history(&actual_tables, &applied).is_ok());
}

#[tokio::test]
#[ignore = "Requires dedicated loopback golf_report_migration_test; never use Neon"]
async fn empty_database_runs_all_migrations() {
    let pool = isolated().await;
    migrate(&pool).await.unwrap();
    assert_retired(&pool).await;
    pool.close().await;
}

#[tokio::test]
#[ignore = "Requires dedicated loopback golf_report_migration_test; never use Neon"]
async fn historical_settings_migration_and_rows_are_preserved_without_runtime_api() {
    let pool = isolated().await;
    migrate(&pool).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM measurement_settings")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    // Historical migration remains applied. Removing the UI must not delete rows.
    sqlx::query("INSERT INTO measurement_settings(product_code,station_type,load_g,push_dist_mm,bend_lower_mm,bend_upper_mm,positions_mm) VALUES('KEEP-QC','TIP',200,8,0,2,ARRAY[80]::float8[])")
        .execute(&pool).await.unwrap();
    let before: serde_json::Value = sqlx::query_scalar(
        "SELECT to_jsonb(s) FROM measurement_settings s WHERE product_code='KEEP-QC'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    migrate(&pool).await.unwrap();
    let after: serde_json::Value = sqlx::query_scalar(
        "SELECT to_jsonb(s) FROM measurement_settings s WHERE product_code='KEEP-QC'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, after);
    pool.close().await;
}

#[tokio::test]
#[ignore = "Requires dedicated loopback golf_report_migration_test; migration, projection and KPI only"]
async fn authoritative_results_preserve_history_and_independent_detail_counters() {
    use crate::{
        database::test_support::registers, models::*, repositories::*, services::InboxService,
    };
    let pool = isolated().await;
    let old = sqlx::migrate::Migrator {
        migrations: Cow::Owned(
            MIGRATOR
                .iter()
                .filter(|m| m.version < 20261118150000)
                .cloned()
                .collect(),
        ),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    old.run(&pool).await.unwrap();
    let source = SourceRepository::upsert(&pool, "authoritative-results-test", None)
        .await
        .unwrap();
    let old_id: i64 = sqlx::query_scalar("INSERT INTO measurements(source_id,session_id,record_seq,total_count,product_code,club_number,measured_at,overall_result,publication_count,quality_error,sku_origin) VALUES($1,'test',1,1,'TEST',1,now(),'UNMEASURED',1,'historical workflow warning','QC') RETURNING id")
        .bind(source.id).fetch_one(&pool).await.unwrap();
    let before: serde_json::Value =
        sqlx::query_scalar("SELECT to_jsonb(m) FROM measurements m WHERE id=$1")
            .bind(old_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    migrate(&pool).await.unwrap();
    let after: serde_json::Value =
        sqlx::query_scalar("SELECT to_jsonb(m) FROM measurements m WHERE id=$1")
            .bind(old_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(before, after);
    for count in [2, 3] {
        let mut raw = registers(count);
        raw[20] = if count == 2 { 1 } else { 2 };
        raw[24] = 2;
        raw[25] = 4;
        raw[26] = 1;
        raw[32] = 2;
        raw[33] = 1;
        raw[34] = 4;
        if count == 3 {
            raw[1] = 0;
        } // Diagnostic cannot overwrite D25040.
        let row: InboxRecord = sqlx::query_as("INSERT INTO plc_inbox(source_id,ingest_key,record_seq,total_count,buffer_index,raw_registers,word_order,received_at,processing_status,publication_session,publication_count,assigned_sku) VALUES($1,$2,$3::integer,$3::bigint,0,$4,'LOW',now(),'PENDING','test',$3::integer,'TEST') RETURNING *")
            .bind(source.id).bind(format!("test-{count}")).bind(count).bind(serde_json::json!(raw))
            .fetch_one(&pool).await.unwrap();
        let id = InboxService::process(&pool, row.id).await.unwrap();
        assert_eq!(InboxService::process(&pool, row.id).await.unwrap(), id);
        let saved = MeasurementRepository::find_by_id(&pool, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            saved.summary.overall_result.as_str(),
            if count == 2 { "OK" } else { "NG" }
        );
        assert_eq!(saved.summary.butt_result.as_deref(), Some("NG"));
        assert_eq!(saved.summary.quality_error.is_some(), count == 3);
        let butt = saved
            .stations
            .iter()
            .find(|s| s.station_type == StationType::Butt)
            .unwrap();
        assert_eq!(butt.point_count, Some(4));
        assert_eq!(butt.ng_point, Some(1));
        assert_eq!(butt.points[0].attempt_count, 1);
        assert_eq!(butt.points[0].decide_attempt, Some(4));
        assert_eq!(butt.points[0].attempts.len(), 4);
        assert_eq!(
            InboxRepository::find(&pool, row.id)
                .await
                .unwrap()
                .raw_registers,
            serde_json::json!(raw)
        );
        assert!(
            sqlx::query("UPDATE measurement_points SET decide_attempt=5 WHERE id=$1")
                .bind(butt.points[0].id)
                .execute(&pool)
                .await
                .is_err()
        );
    }
    let request = MeasurementListRequest::default();
    let stats = MeasurementRepository::statistics(&pool, &request)
        .await
        .unwrap();
    assert_eq!(
        (stats.total_measurements, stats.total_ok, stats.total_ng),
        (2, 1, 1)
    );
    let history = MeasurementRepository::history_page(&pool, &request)
        .await
        .unwrap();
    assert_eq!(history.total, 3);
    pool.close().await;
}
#[tokio::test]
#[ignore = "Requires dedicated loopback golf_report_migration_test; never use Neon"]
async fn existing_empty_registry_upgrades_without_changing_source_or_old_checksums() {
    let pool = isolated().await;
    before_retirement(&pool).await;
    sqlx::query("INSERT INTO plc_sources(source_key) VALUES('migration-test-preserved')")
        .execute(&pool)
        .await
        .unwrap();
    let source: serde_json::Value =
        sqlx::query_scalar("SELECT row_to_json(s)::jsonb FROM plc_sources s")
            .fetch_one(&pool)
            .await
            .unwrap();
    let old: Vec<(i64, Vec<u8>)> =
        sqlx::query_as("SELECT version,checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool)
            .await
            .unwrap();
    migrate(&pool).await.unwrap();
    assert_retired(&pool).await;
    let after: serde_json::Value =
        sqlx::query_scalar("SELECT row_to_json(s)::jsonb FROM plc_sources s")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(source, after);
    let checksums: Vec<(i64, Vec<u8>)> = sqlx::query_as(
        "SELECT version,checksum FROM _sqlx_migrations WHERE version<$1 ORDER BY version",
    )
    .bind(REGISTRY_RETIREMENT_VERSION)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(old, checksums);
    pool.close().await;
}
#[tokio::test]
#[ignore = "Requires dedicated loopback golf_report_migration_test; never use Neon"]
async fn missing_registry_without_retirement_fails_before_runner() {
    let pool = isolated().await;
    before_retirement(&pool).await;
    sqlx::query("DROP TABLE plc_source_registry RESTRICT")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        migrate(&pool)
            .await
            .unwrap_err()
            .contains("plc_source_registry")
    );
    assert_eq!(audit(&pool).await.unwrap().1.len(), 4);
    pool.close().await;
}
#[tokio::test]
#[ignore = "Requires dedicated loopback golf_report_migration_test; never use Neon"]
async fn retired_schema_restarts_without_registry() {
    let pool = isolated().await;
    migrate(&pool).await.unwrap();
    migrate(&pool).await.unwrap();
    assert_retired(&pool).await;
    pool.close().await;
}
#[tokio::test]
#[ignore = "Requires dedicated loopback golf_report_migration_test; never use Neon"]
async fn populated_registry_blocks_retirement_and_preserves_data() {
    let pool = isolated().await;
    before_retirement(&pool).await;
    sqlx::query("WITH s AS (INSERT INTO plc_sources(source_key) VALUES('migration-test-enrolled') RETURNING id) INSERT INTO plc_source_registry(source_id,physical_identity,endpoint_ip,endpoint_port,unit_id,session_id) SELECT id,'keep-evidence','127.0.0.1',502,1,'test-session' FROM s").execute(&pool).await.unwrap();
    let before: serde_json::Value =
        sqlx::query_scalar("SELECT row_to_json(r)::jsonb FROM plc_source_registry r")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        migrate(&pool)
            .await
            .unwrap_err()
            .contains("Registry contains data")
    );
    let after: serde_json::Value =
        sqlx::query_scalar("SELECT row_to_json(r)::jsonb FROM plc_source_registry r")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(before, after);
    assert_eq!(audit(&pool).await.unwrap().1.len(), 4);
    pool.close().await;
}
#[tokio::test]
#[ignore = "Requires dedicated loopback golf_report_migration_test; never use Neon"]
async fn external_view_dependency_blocks_retirement() {
    let pool = isolated().await;
    before_retirement(&pool).await;
    sqlx::query("CREATE VIEW registry_consumer AS SELECT source_id FROM plc_source_registry")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        migrate(&pool)
            .await
            .unwrap_err()
            .contains("Unexpected registry dependency")
    );
    let _: Vec<(i64,)> = sqlx::query_as("SELECT source_id FROM registry_consumer")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(audit(&pool).await.unwrap().1.len(), 4);
    pool.close().await;
}

#[tokio::test]
#[ignore = "Requires dedicated loopback golf_report_migration_test; never use Neon"]
async fn foreign_key_and_row_type_consumers_block_retirement() {
    for ddl in [
        "CREATE TABLE registry_child(source_id bigint REFERENCES plc_source_registry(source_id))",
        "CREATE TABLE registry_child(value plc_source_registry)",
    ] {
        let pool = isolated().await;
        before_retirement(&pool).await;
        sqlx::query(sqlx::AssertSqlSafe(ddl))
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            migrate(&pool)
                .await
                .unwrap_err()
                .contains("Unexpected registry dependency")
        );
        let _: Vec<(i32,)> = sqlx::query_as("SELECT 1 FROM registry_child")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(audit(&pool).await.unwrap().1.len(), 4);
        pool.close().await;
    }
}
