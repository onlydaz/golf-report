use golf_report_lib::database::{config::load_config, migrations};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
use std::str::FromStr;
async fn run() -> Result<(), String> {
    let mut target = None;
    let mut apply = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--database" => target = Some(args.next().ok_or("Thiếu tên database")?),
            "--migrate" => apply = true,
            _ => {
                return Err(
                    "Dùng: as200-db [--database NAME] [--migrate]. Mặc định chỉ audit.".into(),
                );
            }
        }
    }
    if apply && target.is_none() {
        return Err("--migrate bắt buộc chỉ định --database rõ ràng".into());
    }
    let settings = load_config()?;
    let name = target.as_deref().unwrap_or(&settings.database);
    if target.is_some() && settings.connection_string.is_some() {
        return Err("--database không được dùng với connection_string; chọn database trong connection string.".into());
    }
    let cloud = settings.connection_string.is_some();
    let options = if let Some(connection_string) = settings.connection_string.as_deref() {
        PgConnectOptions::from_str(connection_string)
            .map_err(|_| "Connection string PostgreSQL không hợp lệ.".to_string())?
            .ssl_mode(PgSslMode::VerifyFull)
    } else {
        PgConnectOptions::new()
            .host(&settings.host)
            .port(settings.port)
            .database(name)
            .username(&settings.username)
            .password(&settings.password)
    };
    let options = if cloud && !apply {
        options.options([("default_transaction_read_only", "on")])
    } else {
        options
    };
    // The URL can supply the database while settings.database remains empty.
    // Report the connected server identity, never the URL or credentials.
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect_with(options)
        .await
        .map_err(|e| e.to_string())?;
    let (database, schema, search_path, read_only, create_public):
        (String, Option<String>, String, String, bool) = sqlx::query_as(
        "SELECT current_database()::text, current_schema()::text, current_setting('search_path'), \
         current_setting('transaction_read_only'), has_schema_privilege(current_user,'public','CREATE')",
    ).fetch_one(&pool).await.map_err(|e| e.to_string())?;
    println!(
        "Connected database={database}, schema={schema:?}, search_path={search_path}, read_only={read_only}, can_create_public={create_public}"
    );
    let schema_tables: Vec<(String, String)> = sqlx::query_as(
        "SELECT schemaname::text,tablename::text FROM pg_tables \
         WHERE schemaname NOT IN ('pg_catalog','information_schema') ORDER BY 1,2",
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| e.to_string())?;
    println!("Tables by schema: {schema_tables:?}");
    let tracking_columns: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT column_name::text,data_type::text,is_nullable::text FROM information_schema.columns \
         WHERE table_schema=current_schema() AND table_name='_sqlx_migrations' ORDER BY ordinal_position",
    ).fetch_all(&pool).await.map_err(|e| e.to_string())?;
    println!("Migration tracking columns: {tracking_columns:?}");
    let advisory_locks: Vec<(bool, i64)> = sqlx::query_as(
        "SELECT l.granted,count(*) FROM pg_locks l JOIN pg_stat_activity a ON a.pid=l.pid \
         WHERE a.datname=current_database() AND l.locktype='advisory' GROUP BY l.granted",
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| e.to_string())?;
    println!("Advisory locks (granted, count): {advisory_locks:?}");
    println!(
        "Embedded migrations: {:?}",
        migrations::MIGRATOR
            .iter()
            .map(|m| m.version)
            .collect::<Vec<_>>()
    );
    let (tables, history) = migrations::audit(&pool).await?;
    println!("Database={database}, Tables={tables:?}, History={history:?}");
    if apply {
        migrations::migrate(&pool).await?;
        println!(
            "AS200 migration verified: {:?}",
            migrations::audit(&pool).await?
        );
    }
    let (tables, history) = migrations::audit(&pool).await?;
    if !history.is_empty() && migrations::validate_history(&tables, &history).is_ok() {
        let counts:Vec<(String,i64)>=sqlx::query_as("SELECT 'plc_sources'::text,count(*) FROM plc_sources UNION ALL SELECT 'plc_inbox',count(*) FROM plc_inbox UNION ALL SELECT 'measurements',count(*) FROM measurements UNION ALL SELECT 'measurement_stations',count(*) FROM measurement_stations UNION ALL SELECT 'measurement_points',count(*) FROM measurement_points UNION ALL SELECT 'measurement_attempts',count(*) FROM measurement_attempts")
            .fetch_all(&pool).await.map_err(|e|e.to_string())?;
        println!("Business row counts: {counts:?}");
    }
    Ok(())
}
fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    if let Err(error) = runtime.block_on(run()) {
        eprintln!("AS200 database: {error}");
        std::process::exit(1);
    }
}
