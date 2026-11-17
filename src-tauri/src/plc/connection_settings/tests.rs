use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct TestDirectory(PathBuf);
impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "golf-single-plc-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for TestDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn config() -> SimulatorConfig {
    simulator_config::parse(DEFAULT_CONFIG).unwrap()
}

#[test]
fn missing_config_creates_disabled_fixed_profile_without_runtime_or_ack() {
    let dir = TestDirectory::new();
    ensure_config(&dir.0).unwrap();
    let (_, before, cfg) = read_config(&dir.0).unwrap();
    assert_eq!(cfg.address, PLC_ADDRESS);
    assert_eq!(cfg.source_key, SOURCE_KEY);
    assert_eq!(cfg.unit_id, 1);
    assert_eq!(cfg.journal_path.as_deref(), Some(Path::new(JOURNAL)));
    assert!(!cfg.enabled && !cfg.ack_enabled && cfg.read_only && !cfg.technical_setup_confirmed);
    assert!(!dir.0.join("local-runtime").exists());
    ensure_config(&dir.0).unwrap();
    assert_eq!(
        before,
        fs::read_to_string(dir.0.join("as200_config.toml")).unwrap()
    );
}
#[test]
fn missing_config_never_replaces_or_adopts_history() {
    for historical in [
        "local-runtime/capture.json",
        "local-runtime/managed-sources/old/capture.json",
        "as200_capture_journal.json",
    ] {
        let dir = TestDirectory::new();
        let path = dir.0.join(historical);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "old-intent-checkpoint").unwrap();
        assert!(ensure_config(&dir.0).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "old-intent-checkpoint");
        assert!(!dir.0.join("as200_config.toml").exists());
    }
}
#[test]
fn fixed_identity_and_ack_permission_cannot_be_changed_by_connect() {
    let dir = TestDirectory::new();
    ensure_config(&dir.0).unwrap();
    let (path, text, cfg) = read_config(&dir.0).unwrap();
    enable(&path, &text).unwrap();
    let (_, after, enabled) = read_config(&dir.0).unwrap();
    assert!(enabled.enabled);
    assert_eq!(enabled.confirmed_session_id, cfg.confirmed_session_id);
    assert!(!enabled.ack_enabled && enabled.read_only && !enabled.technical_setup_confirmed);
    assert!(enable(&path, &text).is_err()); // stale file cannot overwrite a later edit
    assert_eq!(after, fs::read_to_string(path).unwrap());
    for mutate in 0..4 {
        let mut other = cfg.clone();
        match mutate {
            0 => other.address = "127.0.0.1:1503".into(),
            1 => other.unit_id = 2,
            2 => other.source_key = "endpoint-v1-old".into(),
            _ => other.journal_path = Some("../capture.json".into()),
        }
        assert!(validate_config(&other).is_err());
    }
    let mut unapproved = cfg;
    unapproved.read_only = false;
    unapproved.ack_enabled = true;
    assert!(validate_config(&unapproved).is_err());
    unapproved.technical_setup_confirmed = true;
    assert!(validate_config(&unapproved).is_ok()); // only explicitly configured permission
}
#[test]
fn initial_baseline_rejects_existing_counters_done_or_records() {
    let mut words = vec![0u16; map::CONTROL_SIZE];
    words[0] = 1;
    words[1] = 240;
    words[2] = 2;
    for index in [4, 8, 11, 12] {
        let mut raw = words.clone();
        raw[index] = 1;
        let snapshot = super::super::as200::reader::PublishedRecordSnapshot {
            control: super::super::as200::decode::control(
                &raw,
                super::super::as200::WordOrder::LowWordFirst,
            )
            .unwrap(),
            buffer: None,
            diagnostics: vec![],
        };
        assert!(clean_new_baseline(&snapshot).is_err());
    }
}

#[tokio::test]
async fn lost_journal_or_unresolved_config_never_reaches_database_or_plc() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .unwrap();
    pool.close().await;
    for name in [
        "local-runtime/capture.lock",
        "local-runtime/capture.tmp",
        "as200_config.connection-pending",
    ] {
        let dir = TestDirectory::new();
        let evidence = dir.0.join(name);
        fs::create_dir_all(evidence.parent().unwrap()).unwrap();
        fs::write(&evidence, "preserve").unwrap();
        let error = prepare_collection(&dir.0, &config(), &pool)
            .await
            .unwrap_err();
        assert!(error.contains("đối chiếu"));
        assert_eq!(fs::read_to_string(evidence).unwrap(), "preserve");
        assert!(!dir.0.join(JOURNAL).exists());
    }
}

// A loopback mock accepts FC03 only. No test ever connects to the configured real IP.
async fn mock_plc(valid: bool) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let task = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            loop {
                let mut request = [0u8; 12];
                if stream.read_exact(&mut request).await.is_err() {
                    break;
                }
                assert_eq!(request[7], 3, "inspection must never write");
                assert_eq!(request[6], 1);
                let start = u16::from_be_bytes([request[8], request[9]]);
                let count = u16::from_be_bytes([request[10], request[11]]);
                assert!(count <= 100);
                let mut response = request[..7].to_vec();
                response[4..6].copy_from_slice(&(3 + 2 * count).to_be_bytes());
                response.extend_from_slice(&[3, (2 * count) as u8]);
                for address in start..start + count {
                    let word: u16 = match address {
                        25000 => {
                            if valid {
                                1
                            } else {
                                9
                            }
                        }
                        25001 => 240,
                        25002 => 2,
                        25007 => 42,
                        _ => 0,
                    };
                    response.extend_from_slice(&word.to_be_bytes());
                }
                stream.write_all(&response).await.unwrap();
            }
        }
    });
    (address, task)
}
#[tokio::test]
async fn inspection_is_read_only_and_separates_tcp_modbus_map() {
    for valid in [true, false] {
        let (address, task) = mock_plc(valid).await;
        let mut cfg = config();
        cfg.address = address;
        let report = inspect_endpoint(cfg).await;
        task.abort();
        assert!(report.tcp_connected && report.modbus_read_succeeded);
        assert_eq!(report.as200_valid, valid);
        assert_eq!(report.mapping_warning.is_none(), valid);
    }
}
#[tokio::test]
async fn fresh_baseline_requires_repeated_empty_raw_reads() {
    let (address, task) = mock_plc(true).await;
    let mut cfg = config();
    cfg.address = address;
    read_new_baseline(&cfg).await.unwrap();
    task.abort();
}
