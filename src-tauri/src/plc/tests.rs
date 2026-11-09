//! Kiểm thử tích hợp bằng simulator thật, chỉ ghi vào schema riêng rồi xóa schema đó.
use super::*;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{
    io::Write,
    process::{Child, Command, Stdio},
    time::Duration,
};

struct Simulator(Child);
impl Simulator {
    fn start() -> Self {
        let folder =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/plc-simulator");
        let child = Command::new(folder.join(".venv/Scripts/python.exe"))
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .args([
                "-B",
                "-u",
                "-c",
                "import asyncio, plc_simulator as p; p.PORT=11502; asyncio.run(p.main())",
            ])
            .current_dir(folder)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self(child)
    }
    fn send(&mut self, mode: &str) {
        writeln!(self.0.stdin.as_mut().unwrap(), "{mode}").unwrap();
    }
}
impl Drop for Simulator {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
async fn wait(config: &PlcConfig, sequence: u16) -> [u16; protocol::COUNT] {
    let mut last_error = String::new();
    for _ in 0..40 {
        match protocol::snapshot(config).await {
            Ok(Some(raw)) if raw[1] == sequence => return raw,
            Ok(Some(raw)) => last_error = format!("sequence hiện tại {}", raw[1]),
            Ok(None) => last_error = "Dữ liệu đang thay đổi".into(),
            Err(error) => last_error = error,
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("Không nhận sequence {sequence}: {last_error}");
}

#[tokio::test]
#[ignore = "Cần Python simulator, PostgreSQL và cổng 11502 trống; dùng schema tạm riêng"]
async fn simulator_and_postgres_end_to_end() {
    // Không kết nối nhầm hoặc tác động một simulator khác trên cổng kiểm thử.
    let port_guard =
        std::net::TcpListener::bind("127.0.0.1:11502").expect("Cổng kiểm thử 11502 phải trống");
    drop(port_guard);
    let app_config = std::env::var_os("APPDATA")
        .map(|path| std::path::PathBuf::from(path).join("com.golf.report"));
    let config =
        crate::database::config::load_config(app_config).expect("Cần database_config.toml");
    let options = PgConnectOptions::new()
        .host(&config.host)
        .port(config.port)
        .database(&config.database)
        .username(&config.username)
        .password(&config.password);
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await
        .unwrap();
    let schema = format!("plc_test_{}", chrono::Utc::now().timestamp_micros());
    assert!(
        schema
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    );
    sqlx::QueryBuilder::<sqlx::Postgres>::new("CREATE SCHEMA ")
        .push(&schema)
        .build()
        .execute(&admin)
        .await
        .unwrap();
    let path = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .after_connect(move |connection, _| {
            let path = path.clone();
            Box::pin(async move {
                sqlx::query("SELECT set_config('search_path',$1,false)")
                    .bind(path)
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap();
    let result = tokio::spawn(async move {
        sqlx::raw_sql(include_str!("../../migrations/20261104120000_create_measurements.sql")).execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261105120000_plc_inbox.sql")).execute(&pool).await.unwrap();
        let database = DatabaseState::with_test_pool(pool.clone()).await;
        let mut state = PlcState::default();
        state.config.address = "127.0.0.1:11502".into();
        let mut simulator = Simulator::start();
        wait(&state.config, 0).await;
        // Không có mã: không lưu. Xác nhận mã sau đó cũng không nhận kết quả cũ.
        simulator.send("1"); wait(&state.config, 1).await;
        state.tick(&database).await.unwrap();
        begin_capture("TEST-PRODUCT", &state, &database).await.unwrap();
        for _ in 0..3 { state.tick(&database).await.unwrap(); }
        let count: (i64,) = sqlx::query_as("SELECT count(*) FROM measurements").fetch_one(&pool).await.unwrap();
        assert_eq!(count.0, 0);
        for (sequence, mode, overall) in [(2, "1", "OK"), (3, "2", "NG"), (4, "3", "NG")] {
            simulator.send(mode); wait(&state.config, sequence).await;
            state.tick(&database).await.unwrap();
            assert!(begin_capture("CHANGED", &state, &database).await.is_err());
            state.tick(&database).await.unwrap();
            let status = state.status.lock().await.clone();
            assert_eq!(status.phase, "SAVED");
            let saved: (String,) = sqlx::query_as("SELECT overall_result FROM measurements WHERE id=$1")
                .bind(status.measurement_id.unwrap()).fetch_one(&pool).await.unwrap();
            assert_eq!(saved.0, overall);
            for _ in 0..3 { state.tick(&database).await.unwrap(); }
        }
        let skipped: (i64,) = sqlx::query_as("SELECT count(*) FROM tip_positions WHERE result='SKIPPED' AND final_distance IS NULL AND retry_count IS NULL")
            .fetch_one(&pool).await.unwrap();
        assert!(skipped.0 > 0);
        // Tái đọc cùng inbox sau commit (mô phỏng mất phản hồi): phải trả đúng ID cũ.
        let (generation, sequence, id): (i64, i32, i64) = sqlx::query_as("SELECT generation,sequence_id,measurement_id FROM plc_inbox ORDER BY sequence_id DESC LIMIT 1")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(storage::save(&pool, &state.config.source, generation, sequence).await.unwrap().0, id);
        // Dữ liệu chờ bền vững được khôi phục bởi worker mới, không cần gán lại mã.
        simulator.send("1"); wait(&state.config, 5).await;
        state.tick(&database).await.unwrap();
        let mut restarted = PlcState::default();
        restarted.config.address = state.config.address.clone();
        restarted.tick(&database).await.unwrap();
        assert_eq!(restarted.status.lock().await.phase, "SAVED");
        restarted.tick(&database).await.unwrap();
        let count: (i64,) = sqlx::query_as("SELECT count(*) FROM measurements").fetch_one(&pool).await.unwrap();
        assert_eq!(count.0, 4);
        // Chặn INSERT để mô phỏng lưu thất bại; inbox phải còn và retry chỉ tạo một phép đo.
        begin_capture("RETRY", &restarted, &database).await.unwrap();
        simulator.send("3"); wait(&state.config, 6).await;
        restarted.tick(&database).await.unwrap();
        sqlx::raw_sql("CREATE FUNCTION fail_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'test failure'; END $$; CREATE TRIGGER test_failure BEFORE INSERT ON measurements FOR EACH ROW EXECUTE FUNCTION fail_insert();")
            .execute(&pool).await.unwrap();
        assert!(restarted.tick(&database).await.is_err());
        assert!(storage::pending(&pool, &state.config.source).await.unwrap().is_some());
        assert!(begin_capture("WRONG", &restarted, &database).await.is_err());
        sqlx::query("DROP TRIGGER test_failure ON measurements").execute(&pool).await.unwrap();
        restarted.tick(&database).await.unwrap();
        assert_eq!(restarted.status.lock().await.phase, "SAVED");
        drop(simulator);
        assert!(restarted.tick(&database).await.is_err());
        assert_eq!(restarted.status.lock().await.connection, "DISCONNECTED");
        let mut simulator = Simulator::start(); wait(&state.config, 0).await;
        restarted.tick(&database).await.unwrap();
        simulator.send("1"); wait(&state.config, 1).await;
        restarted.tick(&database).await.unwrap(); restarted.tick(&database).await.unwrap();
        assert_eq!(restarted.status.lock().await.phase, "SAVED");
        let count: (i64,) = sqlx::query_as("SELECT count(*) FROM measurements").fetch_one(&pool).await.unwrap();
        assert_eq!(count.0, 6);
        assert_eq!(storage::observe(&pool, &state.config.source, 65535).await.unwrap(), 1);
        assert_eq!(storage::observe(&pool, &state.config.source, 1).await.unwrap(), 2);
        pool.close().await;
    }).await;
    // Chỉ xóa schema do chính kiểm thử này tạo, kể cả khi assertion thất bại.
    sqlx::QueryBuilder::<sqlx::Postgres>::new("DROP SCHEMA ")
        .push(&schema)
        .push(" CASCADE")
        .build()
        .execute(&admin)
        .await
        .unwrap();
    result.unwrap();
}
