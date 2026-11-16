//! Explicitly opt-in integration infrastructure. Never connects to the application database.
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};

pub(crate) fn options() -> PgConnectOptions {
    assert_eq!(
        std::env::var("AS200_TEST_DATABASE").as_deref(),
        Ok("golf_report_test")
    );
    let settings = super::config::load_config().unwrap();
    assert!(
        matches!(settings.host.as_str(), "localhost" | "127.0.0.1" | "::1"),
        "Only the explicitly authorized local PostgreSQL test server is allowed"
    );
    println!(
        "TEST endpoint={}:{} target=golf_report_test role={} (password redacted)",
        settings.host, settings.port, settings.username
    );
    PgConnectOptions::new()
        .host(&settings.host)
        .port(settings.port)
        .username(&settings.username)
        .password(&settings.password)
        .database("golf_report_test")
        .application_name("golf-report-prompt13-test")
}
pub(crate) async fn verify(pool: &PgPool) {
    let (database, host, port, role, writable):(String,String,i32,String,bool)=sqlx::query_as(
        "SELECT current_database()::text,host(inet_server_addr()),inet_server_port(),current_user::text,NOT current_setting('transaction_read_only')::boolean")
        .fetch_one(pool).await.unwrap();
    println!("VERIFIED database={database} server={host}:{port} role={role} writable={writable}");
    assert_eq!(database, "golf_report_test");
    assert!(writable);
    assert!(matches!(
        host.as_str(),
        "127.0.0.1" | "::1" | "::ffff:127.0.0.1"
    ));
}
pub(crate) async fn pool(max: u32) -> PgPool {
    let pool = PgPoolOptions::new()
        .max_connections(max)
        .connect_with(options())
        .await
        .unwrap();
    verify(&pool).await;
    let allowed:bool=sqlx::query_scalar("SELECT bool_and(has_table_privilege(current_user,tablename,'SELECT,INSERT,UPDATE')) FROM pg_tables WHERE schemaname='public' AND tablename <> '_sqlx_migrations'")
        .fetch_one(&pool).await.unwrap();
    assert!(allowed, "Integration role lacks table permissions");
    pool
}

#[tokio::test]
#[ignore = "Explicit AS200_TEST_DATABASE=golf_report_test only; creates test DB if absent, migrates without reset"]
async fn prepare_postgres_integration_database() {
    let maintenance = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options().database("postgres"))
        .await
        .unwrap();
    let maintenance_name: String = sqlx::query_scalar("SELECT current_database()::text")
        .fetch_one(&maintenance)
        .await
        .unwrap();
    assert_eq!(maintenance_name, "postgres");
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname='golf_report_test')",
    )
    .fetch_one(&maintenance)
    .await
    .unwrap();
    if !exists {
        let can_create: bool = sqlx::query_scalar(
            "SELECT rolcreatedb OR rolsuper FROM pg_roles WHERE rolname=current_user",
        )
        .fetch_one(&maintenance)
        .await
        .unwrap();
        assert!(
            can_create,
            "No permission to create isolated test database; do not use another database"
        );
        sqlx::query("CREATE DATABASE golf_report_test")
            .execute(&maintenance)
            .await
            .unwrap();
        println!("CREATED golf_report_test; no production data copied");
    } else {
        println!("EXISTING golf_report_test: preserving all existing objects and data");
    }
    maintenance.close().await;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options())
        .await
        .unwrap();
    verify(&pool).await;
    let (tables, history) = super::migrations::audit(&pool).await.unwrap();
    println!("BEFORE migration tables={tables:?} history={history:?}");
    super::migrations::migrate(&pool).await.unwrap();
    let (tables, history) = super::migrations::audit(&pool).await.unwrap();
    assert_eq!(tables.len(), 7);
    assert_eq!(history.len(), 1);
    assert!(history[0].success);
    let constraints:Vec<String>=sqlx::query_scalar("SELECT conname::text FROM pg_constraint WHERE connamespace='public'::regnamespace ORDER BY conname").fetch_all(&pool).await.unwrap();
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT indexname::text FROM pg_indexes WHERE schemaname='public' ORDER BY indexname",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(indexes.iter().any(|n| n == "inbox_confirmed_sequence"));
    assert!(
        indexes
            .iter()
            .any(|n| n == "measurements_confirmed_sequence")
    );
    assert!(constraints.len() > 20);
    let version: String = sqlx::query_scalar("SHOW server_version")
        .fetch_one(&pool)
        .await
        .unwrap();
    let counts:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM plc_sources),(SELECT count(*) FROM plc_inbox),(SELECT count(*) FROM measurements)").fetch_one(&pool).await.unwrap();
    println!(
        "SCHEMA PostgreSQL={version} tables={tables:?} history={history:?} counts(source,inbox,measurement)={counts:?}\nCONSTRAINTS={constraints:?}\nINDEXES={indexes:?}"
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "SELECT audit on verified golf_report_test after DML; no cleanup or reset"]
async fn postgres_prompt13_final_database_audit() {
    let pool = pool(1).await;
    let counts:(i64,i64,i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM plc_sources),(SELECT count(*) FROM plc_inbox),(SELECT count(*) FROM measurements),(SELECT count(*) FROM measurement_stations),(SELECT count(*) FROM measurement_points),(SELECT count(*) FROM measurement_attempts)").fetch_one(&pool).await.unwrap();
    let states:Vec<(String,i64)>=sqlx::query_as("SELECT processing_status::text,count(*) FROM plc_inbox GROUP BY processing_status ORDER BY processing_status").fetch_all(&pool).await.unwrap();
    let fixtures:Vec<(String,i64,i64)>=sqlx::query_as("SELECT s.source_key::text,(SELECT count(*) FROM plc_inbox i WHERE i.source_id=s.id),(SELECT count(*) FROM measurements m WHERE m.source_id=s.id) FROM plc_sources s ORDER BY s.id").fetch_all(&pool).await.unwrap();
    let duplicates:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM (SELECT source_id,session_id,record_seq FROM measurements WHERE session_id IS NOT NULL GROUP BY 1,2,3 HAVING count(*)>1) x)+(SELECT count(*) FROM (SELECT source_id,session_id,record_seq FROM plc_inbox WHERE session_id IS NOT NULL GROUP BY 1,2,3 HAVING count(*)>1) y)").fetch_one(&pool).await.unwrap();
    let invalid_links:i64=sqlx::query_scalar("SELECT count(*) FROM plc_inbox i LEFT JOIN measurements m ON m.id=i.measurement_id WHERE i.processing_status='PROCESSED' AND (m.id IS NULL OR i.source_id<>m.source_id OR i.session_id IS DISTINCT FROM m.session_id OR i.record_seq<>m.record_seq)").fetch_one(&pool).await.unwrap();
    let partial:i64=sqlx::query_scalar("SELECT count(*) FROM measurements m WHERE (SELECT count(*) FROM measurement_stations s WHERE s.measurement_id=m.id)<>2 OR (SELECT count(*) FROM measurement_points p JOIN measurement_stations s ON s.id=p.station_id WHERE s.measurement_id=m.id)<>8").fetch_one(&pool).await.unwrap();
    let invalid_attempts:i64=sqlx::query_scalar("SELECT count(*) FROM measurement_points p WHERE (SELECT count(*) FROM measurement_attempts a WHERE a.point_id=p.id)<>p.attempt_count").fetch_one(&pool).await.unwrap();
    let stray_triggers: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_trigger WHERE NOT tgisinternal AND tgname='as200_test_failure'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (
            duplicates,
            invalid_links,
            partial,
            invalid_attempts,
            stray_triggers
        ),
        (0, 0, 0, 0, 0)
    );
    println!(
        "FINAL counts sources/inbox/measurements/stations/points/attempts={counts:?}\nSTATES={states:?}\nFIXTURES={fixtures:?}\nDUPLICATES={duplicates} INVALID_LINKS={invalid_links} PARTIAL_TREES={partial} INVALID_ATTEMPT_COUNTS={invalid_attempts} LEFTOVER_TRIGGERS={stray_triggers}"
    );
    pool.close().await;
}
