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
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect_with(options)
        .await
        .map_err(|e| e.to_string())?;
    let (tables, history) = migrations::audit(&pool).await?;
    println!("Database={name}, Tables={tables:?}, History={history:?}");
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
        let registry: (i64, i64, i64) = sqlx::query_as(
            "SELECT count(*), count(DISTINCT session_id), count(*) FILTER (WHERE ack_write_approved) FROM plc_source_registry",
        )
        .fetch_one(&pool)
        .await
        .map_err(|e| e.to_string())?;
        println!(
            "Source registry: rows={}, sessions={}, ack_approved={}",
            registry.0, registry.1, registry.2
        );
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
