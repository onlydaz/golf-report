//! Explicit opt-in Neon integration tests. No provisioning, migration or automatic cleanup.
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions, PgSslMode},
};
use std::str::FromStr;

pub(crate) fn run_id() -> String {
    let run = std::env::var("GOLF_NEON_TEST_RUN").expect("Set unique GOLF_NEON_TEST_RUN");
    assert!(
        run.starts_with("grit-")
            && run.len() <= 32
            && run.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    );
    run
}
pub(crate) fn session() -> String {
    format!("{}-session", run_id())
}
pub(crate) fn sku() -> String {
    format!("IT-{}", run_id())
}
pub(crate) fn source_key(label: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let key = format!(
        "{}-{label}-{}",
        run_id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    );
    assert!(key.len() <= 100);
    println!("TEST_SOURCE key={key} session={}", session());
    key
}
pub(crate) async fn pool(max: u32) -> PgPool {
    verified_pool(max, false).await
}
pub(crate) async fn readonly_pool(max: u32) -> PgPool {
    verified_pool(max, true).await
}
async fn verified_pool(max: u32, readonly: bool) -> PgPool {
    assert_eq!(
        std::env::var("GOLF_NEON_INTEGRATION").as_deref(),
        Ok("I_ACCEPT_golf_report")
    );
    let config = super::config::load_config().expect("Cannot load configured database file");
    let url = config
        .connection_string
        .as_deref()
        .expect("Neon URL required");
    let options =
        PgConnectOptions::from_str(url).unwrap_or_else(|_| panic!("Invalid DB URL (redacted)"));
    assert_eq!(
        options.get_host(),
        "ep-long-shape-azaeb4rd.c-3.ap-southeast-1.aws.neon.tech"
    );
    assert_eq!(options.get_database(), Some("golf_report"));
    assert_eq!(options.get_port(), 5432);
    let options = options
        .ssl_mode(PgSslMode::VerifyFull)
        .application_name("golf-report-neon-integration")
        .options([
            ("search_path", "public"),
            ("statement_timeout", "15000"),
            ("lock_timeout", "5000"),
        ]);
    let options = if readonly {
        options.options([("default_transaction_read_only", "on")])
    } else {
        options
    };
    let pool = PgPoolOptions::new()
        .max_connections(max)
        .acquire_timeout(std::time::Duration::from_secs(20))
        .connect_with(options)
        .await
        .unwrap_or_else(|_| panic!("Cannot connect verified Neon target (credentials omitted)"));
    let (db,schema,ro):(String,String,String)=sqlx::query_as("SELECT current_database()::text,current_schema()::text,current_setting('transaction_read_only')")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(db, "golf_report");
    assert_eq!(schema, "public");
    assert_eq!(ro, if readonly { "on" } else { "off" });
    let (tables, history) = super::migrations::audit(&pool).await.unwrap();
    super::migrations::validate_history(&tables, &history).unwrap();
    assert_eq!(history.len(), super::migrations::MIGRATOR.iter().count());
    assert!(history.iter().all(|m| m.success));
    for m in super::migrations::MIGRATOR.iter() {
        assert!(history.iter().any(|h| h.version == m.version));
    }
    verify_untouched(&pool).await;
    println!(
        "TEST_TARGET database={db} schema={schema} readonly={readonly} run={}",
        run_id()
    );
    let inventory:serde_json::Value=sqlx::query_scalar("SELECT coalesce(jsonb_agg(jsonb_build_object('id',s.id,'sourceKey',s.source_key,'session',(SELECT session_id FROM plc_acquisition WHERE source_id=s.id),'inbox',(SELECT count(*) FROM plc_inbox WHERE source_id=s.id),'measurements',(SELECT count(*) FROM measurements WHERE source_id=s.id)) ORDER BY s.id),'[]'::jsonb) FROM plc_sources s WHERE starts_with(s.source_key,$1)")
        .bind(format!("{}-",run_id())).fetch_one(&pool).await.unwrap();
    println!("TEST_INVENTORY {inventory}");
    let counts:serde_json::Value=sqlx::query_scalar("WITH s AS (SELECT id FROM plc_sources WHERE starts_with(source_key,$1)),m AS (SELECT id FROM measurements WHERE source_id IN(SELECT id FROM s)),st AS (SELECT id FROM measurement_stations WHERE measurement_id IN(SELECT id FROM m)),p AS (SELECT id FROM measurement_points WHERE station_id IN(SELECT id FROM st)) SELECT jsonb_build_object('plc_sources',(SELECT count(*) FROM s),'plc_inbox',(SELECT count(*) FROM plc_inbox WHERE source_id IN(SELECT id FROM s)),'plc_acquisition',(SELECT count(*) FROM plc_acquisition WHERE source_id IN(SELECT id FROM s)),'measurements',(SELECT count(*) FROM m),'measurement_stations',(SELECT count(*) FROM st),'measurement_points',(SELECT count(*) FROM p),'measurement_attempts',(SELECT count(*) FROM measurement_attempts WHERE point_id IN(SELECT id FROM p)))")
        .bind(format!("{}-",run_id())).fetch_one(&pool).await.unwrap();println!("TEST_TABLE_COUNTS {counts}");

    pool
}
async fn verify_untouched(pool: &PgPool) {
    let mut snapshot = serde_json::Map::new();
    // Hash every pre-existing source's rows and tracking. No payload/credential is printed.
    for (table, joins, key) in [
        ("plc_sources", "", "t.id"),
        (
            "plc_acquisition",
            " JOIN plc_sources s ON s.id=t.source_id",
            "t.source_id",
        ),
        (
            "plc_inbox",
            " JOIN plc_sources s ON s.id=t.source_id",
            "t.id",
        ),
        (
            "measurements",
            " JOIN plc_sources s ON s.id=t.source_id",
            "t.id",
        ),
        (
            "measurement_stations",
            " JOIN measurements m ON m.id=t.measurement_id JOIN plc_sources s ON s.id=m.source_id",
            "t.id",
        ),
        (
            "measurement_points",
            " JOIN measurement_stations st ON st.id=t.station_id JOIN measurements m ON m.id=st.measurement_id JOIN plc_sources s ON s.id=m.source_id",
            "t.id",
        ),
        (
            "measurement_attempts",
            " JOIN measurement_points p ON p.id=t.point_id JOIN measurement_stations st ON st.id=p.station_id JOIN measurements m ON m.id=st.measurement_id JOIN plc_sources s ON s.id=m.source_id",
            "t.id",
        ),
    ] {
        let source = if table == "plc_sources" { "t" } else { "s" };
        let sql = format!(
            "SELECT jsonb_build_object('count',count(*),'hash',md5(coalesce(string_agg(row_to_json(t)::text,'' ORDER BY {key}),''))) FROM public.{table} t{joins} WHERE NOT starts_with({source}.source_key,$1)"
        );
        let value: serde_json::Value = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(format!("{}-", run_id()))
            .fetch_one(pool)
            .await
            .unwrap();
        snapshot.insert(table.into(), value);
    }
    let value:serde_json::Value=sqlx::query_scalar("SELECT jsonb_build_object('count',count(*),'hash',md5(coalesce(string_agg(row_to_json(t)::text,'' ORDER BY version),''))) FROM public._sqlx_migrations t").fetch_one(pool).await.unwrap();
    snapshot.insert("_sqlx_migrations".into(), value);
    let path = std::env::temp_dir().join(format!("{}-baseline.json", run_id()));
    let snapshot = serde_json::Value::Object(snapshot);
    if path.exists() {
        let before: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            snapshot, before,
            "Rows outside this run changed; investigate concurrent activity, never overwrite production"
        );
    } else {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.write_all(&serde_json::to_vec_pretty(&snapshot).unwrap())
            .unwrap();
    }
    println!("PRODUCTION_SNAPSHOT_UNCHANGED {snapshot}");
}

/// Valid completed NG at TIP P1, BUTT P1-P4 OK; unused TIP slots stay zero.
pub(crate) fn registers(seq: i32) -> Vec<u16> {
    let mut raw = vec![0; 240];
    raw[0] = 1;
    raw[1] = 1;
    raw[2] = 42;
    raw[4] = 0x4241;
    raw[20] = 2;
    raw[22] = seq as u16;
    raw[23] = (seq as u32 >> 16) as u16;
    for (header, base, count, load) in [(24, 32, 4, 125.5_f32), (120, 128, 1, 165.25_f32)] {
        raw[header] = if header == 120 { 2 } else { 1 };
        raw[header + 1] = count;
        raw[header + 2] = if header == 120 { 1 } else { 0 };
        let bits = load.to_bits();
        raw[header + 4] = bits as u16;
        raw[header + 5] = (bits >> 16) as u16;
        for point in 0..usize::from(count) {
            let p = base + point * 22;
            raw[p] = if header == 120 { 2 } else { 1 };
            raw[p + 1] = 4;
            raw[p + 2] = 4;
            for a in 0..4 {
                for (offset, value) in [
                    (6 + a * 4, 1.25_f32 + a as f32),
                    (8 + a * 4, -0.5_f32 - a as f32),
                ] {
                    let bits = value.to_bits();
                    raw[p + offset] = bits as u16;
                    raw[p + offset + 1] = (bits >> 16) as u16;
                }
            }
        }
    }
    raw
}

pub(crate) fn receipt(seq: i32) -> crate::models::InboxRecord {
    crate::models::InboxRecord {
        publication_session: Some("test-session".into()),
        publication_count: Some(seq),
        assigned_sku: Some("SKU-FROZEN".into()),
        decode_error: None,
        id: i64::from(seq) + 1,
        source_id: 1,
        session_id: None,
        ingest_key: format!("publication-v1:12:test-session:{seq}"),
        record_seq: seq,
        total_count: Some(i64::from(seq)),
        buffer_index: 0,
        raw_registers: serde_json::to_value(registers(seq)).unwrap(),
        word_order: "LOW".into(),
        received_at: chrono::Utc::now(),
        processing_status: "SESSION_UNCONFIRMED".into(),
        measurement_id: None,
        error_message: None,
        processed_at: None,
    }
}
