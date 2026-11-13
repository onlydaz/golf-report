use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn empty_new_snapshot() -> super::super::as200::reader::PollReport {
    use super::super::as200::{decode, WordOrder, reader::{PollReport, BufferSnapshot}};
    let mut control = vec![0_u16; 20];
    control[0] = 1; control[1] = 240; control[2] = 2;
    PollReport {
        control: decode::control(&control, WordOrder::LowWordFirst).unwrap(),
        buffers: (0..2).map(|buffer_index| BufferSnapshot { buffer_index, raw_registers: vec![0;240], record: None }).collect(),
        new_records: Vec::new(), diagnostics: Vec::new(),
    }
}

#[test]
fn controlled_new_source_requires_zero_counters_empty_slots_and_no_done() {
    let report = empty_new_snapshot();
    assert!(clean_new_baseline(&report).is_ok());
    for field in 0..5 {
        let mut report = empty_new_snapshot();
        match field {
            0 => report.control.total_count = 1,
            1 => report.control.pc_read_count = 1,
            2 => report.control.pc_read_done = 1,
            3 => report.control.data_ready = 1,
            _ => report.buffers[1].raw_registers[22] = 123,
        }
        assert!(clean_new_baseline(&report).is_err());
    }
}

#[test]
fn legacy_profile_is_resolved_from_later_backup_without_inventing_identity() {
    let dir = Fixture::new();
    let file = dir.0.join("as200_config.toml");
    let first = dir.0.join("local-runtime/managed-sources/first");
    let second = dir.0.join("local-runtime/managed-sources/second");
    std::fs::create_dir_all(&first).unwrap(); std::fs::create_dir_all(&second).unwrap();
    let profile = |address: &str, source: &str, journal: &Path| format!("mode='modbus'\nenabled=true\ntechnical_setup_confirmed=true\naddress='{address}'\nunit_id=1\nword_order='LOW'\nmodbus_address_offset=0\nplc_heartbeat_timeout_ms=5000\nsource_key='{source}'\nconfirmed_session_id='{source}-session'\njournal_path='{}'\n", journal.display());
    let a = profile("192.168.1.24:1504", "first", &first.join("capture.json"));
    let b = profile("127.0.0.1:1503", "second", &second.join("capture.json"));
    std::fs::write(&file, &b).unwrap();
    std::fs::write(second.join("previous-config.toml"), &a).unwrap();
    let requested = Endpoint { ip: "192.168.1.24".into(), port: 1504, unit_id: 1 };
    let historical = previous_endpoint(&file, &requested).unwrap().unwrap();
    assert_eq!(historical.config.source_key, "first");
    assert_eq!(historical.config.confirmed_session_id.as_deref(), Some("first-session"));
    assert_eq!(historical.config.journal_path.unwrap(), first.canonicalize().unwrap().join("capture.json"));
    assert!(previous_endpoint(&file, &Endpoint { ip: "127.0.0.2".into(), port: 1504, unit_id: 1 }).unwrap().is_none());
    retain_current_profile(&file, &b, &simulator_config::parse(&b).unwrap()).unwrap();
    std::fs::write(&file, &a).unwrap();
    let returning = previous_endpoint(&file, &Endpoint { ip: "127.0.0.1".into(), port: 1503, unit_id: 1 }).unwrap().unwrap();
    assert_eq!(returning.config.source_key, "second");
    assert_eq!(std::fs::read_to_string(second.join("target-config.toml")).unwrap(), b);
    std::fs::write(&file, &b).unwrap();
    std::fs::remove_file(second.join("previous-config.toml")).unwrap();
    assert!(previous_endpoint(&file, &requested).is_err());
}

#[test]
fn endpoint_links_revisit_three_profiles_and_never_guess_between_legacy_identities() {
    let dir = Fixture::new();
    let file = dir.0.join("as200_config.toml");
    let mut profiles = Vec::new();
    for (name, port) in [("one", 1505), ("two", 1506), ("three", 1507), ("legacy-one", 1505)] {
        let folder = dir.0.join("local-runtime/managed-sources").join(name);
        std::fs::create_dir_all(&folder).unwrap();
        let text = format!("mode='modbus'\nenabled=true\ntechnical_setup_confirmed=true\naddress='127.0.0.1:{port}'\nunit_id=1\nword_order='LOW'\nmodbus_address_offset=0\nplc_heartbeat_timeout_ms=5000\nsource_key='{name}'\nconfirmed_session_id='{name}-session'\njournal_path='{}'\n", folder.join("capture.json").display());
        std::fs::write(folder.join("target-config.toml"), &text).unwrap();
        profiles.push((text.clone(), simulator_config::parse(&text).unwrap()));
    }
    for index in [0_usize, 1, 2, 0] {
        let (text, config) = &profiles[index];
        std::fs::write(&file, text).unwrap();
        remember_endpoint(&file, text, config).unwrap();
        let selected = previous_endpoint(&file, &endpoint(config).unwrap()).unwrap().unwrap();
        assert_eq!(selected.config.source_key, config.source_key);
        assert_eq!(selected.config.confirmed_session_id, config.confirmed_session_id);
        assert_eq!(selected.config.journal_path, Some(profile_journal(&file, config).unwrap()));
    }
    let one = endpoint(&profiles[0].1).unwrap();
    std::fs::remove_file(endpoint_link(&file, &one).unwrap()).unwrap();
    assert_eq!(previous_endpoint(&file, &one).err().unwrap(), "HISTORY_AMBIGUOUS");
}

#[tokio::test]
async fn controlled_baseline_reads_full_snapshot_using_only_fc03() {
    for total in [0_u16, 1_u16] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            for _ in 0..11 {
                let mut request = [0;12];
                socket.read_exact(&mut request).await.unwrap();
                assert_eq!(request[7], 3, "No Modbus write allowed during bootstrap");
                let offset = u16::from_be_bytes([request[8], request[9]]) as usize - 25000;
                let count = u16::from_be_bytes([request[10], request[11]]) as usize;
                let mut words = vec![0_u16;500];
                words[0] = 1; words[1] = 240; words[2] = 2; words[4] = total;
                let mut response = request[..7].to_vec();
                response[4..6].copy_from_slice(&((count*2+3) as u16).to_be_bytes());
                response.push(3); response.push((count*2) as u8);
                for word in &words[offset..offset+count] { response.extend_from_slice(&word.to_be_bytes()); }
                socket.write_all(&response).await.unwrap();
            }
        });
        let config = SimulatorConfig { address, ..Default::default() };
        let result = read_new_baseline(&config).await;
        assert_eq!(result.is_ok(), total == 0);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn separate_journal_uses_engine_initialization_and_never_reuses_an_existing_path() {
    let dir = Fixture::new();
    let path = dir.0.join("new-source/capture.json");
    let config = SimulatorConfig { enabled: true, mode: "modbus".into(), technical_setup_confirmed: true,
        source_key: "controlled-new-source".into(), confirmed_session_id: Some("controlled-new-session".into()),
        journal_path: Some(path.clone()), ..Default::default() };
    WorkerState::initialize_separate_journal(config.clone(), path.clone()).await.unwrap();
    let before = std::fs::read(&path).unwrap();
    let journal: serde_json::Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(journal["version"], 2);
    assert_eq!(journal["source_key"], config.source_key);
    assert!(journal["ack"]["intent"].is_null());
    assert_eq!(journal["ack"]["accepted"], 0);
    assert_eq!(journal["last_total"], 0);
    assert!(WorkerState::initialize_separate_journal(config, path.clone()).await.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[tokio::test]
async fn interrupted_transition_blocks_autostart_and_legacy_endpoint_save() {
    let dir = Fixture::new();
    let state = dir.state().await;
    let file = dir.0.join("as200_config.toml");
    std::fs::write(file.with_extension("connection-pending"), TEXT).unwrap();
    state.clone().launch(Some(dir.0.clone()), crate::database::DatabaseState::default()).await;
    assert_eq!(state.status.read().await.state, "STOPPED");
    assert!(!state.status.read().await.enabled);
    let result = save(&state, Endpoint { ip: "127.0.0.1".into(), port: 1504, unit_id: 1 }, &revision(TEXT)).await;
    assert!(result.err().unwrap().contains("chưa hoàn tất"));
    assert_eq!(std::fs::read_to_string(file).unwrap(), TEXT);
}
const TEXT: &str = "enabled = false\nmode = \"simulator\"\naddress = \"127.0.0.1:1503\" # endpoint\nunit_id = 1 # unit\nsource_key = \"settings-fixture\"\nconfirmed_session_id = \"confirmed-test-session\"\nword_order = \"HIGH\"\nmodbus_address_offset = -1\ntimeout_ms = 100\nack_enabled = false\npc_heartbeat_enabled = false\n";
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "golf_settings_{}_{}_{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    async fn state(&self) -> WorkerState {
        let state = WorkerState::default();
        *state.settings_path.lock().await = Some(self.0.join("as200_config.toml"));
        state.settings_ready.store(true, Ordering::SeqCst);
        std::fs::write(self.0.join("as200_config.toml"), TEXT).unwrap();
        std::fs::write(self.0.join("as200_capture_journal.lock"), "").unwrap();
        self.journal(serde_json::json!({}));
        state
    }
    fn journal(&self, extra: serde_json::Value) {
        let mut value = serde_json::json!({"version":2,"source_key":"settings-fixture","session_id":"confirmed-test-session","last_total":null,"pause":null,"warnings":[],"deliveries":[]});
        for (k, v) in extra.as_object().unwrap() {
            value[k] = v.clone();
        }
        std::fs::write(self.0.join("as200_capture_journal.json"), value.to_string()).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.0.parent() == Some(std::env::temp_dir().as_path())
            && self
                .0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("golf_settings_")
        {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
fn value() -> Endpoint {
    Endpoint {
        ip: "127.0.0.2".into(),
        port: 1504,
        unit_id: 2,
    }
}

#[test]
fn validation_and_exact_toml_preservation() {
    let cfg = simulator_config::parse(TEXT).unwrap();
    for ip in ["abc", "127.0.0.999"] {
        assert!(
            Endpoint {
                ip: ip.into(),
                ..value()
            }
            .apply(&cfg)
            .is_err()
        );
    }
    for port in [0, 65536] {
        assert!(Endpoint { port, ..value() }.apply(&cfg).is_err());
    }
    for unit_id in [0, 248, 256] {
        assert!(Endpoint { unit_id, ..value() }.apply(&cfg).is_err());
    }
    let next = value().apply(&cfg).unwrap();
    let extended = format!(
        "{TEXT}\n# future policy\nfuture_flag = true\n[future]\naddress = 'do not edit'\nvalues = [1, 2, 3]\n"
    );
    let changed = patch(&extended, &next).unwrap();
    assert_eq!(
        changed,
        extended
            .replace("\"127.0.0.1:1503\"", "\"127.0.0.2:1504\"")
            .replace("unit_id = 1 #", "unit_id = 2 #")
    );
    let windows = format!("# Cấu hình QC\r\n{}", extended.replace('\n', "\r\n"));
    assert_eq!(
        patch(&windows, &next).unwrap(),
        windows
            .replace("\"127.0.0.1:1503\"", "\"127.0.0.2:1504\"")
            .replace("unit_id = 1 #", "unit_id = 2 #")
    );
    let cfg2 = simulator_config::parse(&patch(TEXT, &next).unwrap()).unwrap();
    assert_eq!(cfg2.mode, cfg.mode);
    assert_eq!(cfg2.source_key, cfg.source_key);
    assert_eq!(cfg2.confirmed_session_id, cfg.confirmed_session_id);
    assert!(!cfg2.ack_enabled);
    assert!(!cfg2.pc_heartbeat_enabled);
    assert!(!cfg2.enabled);
    assert!(!cfg2.allow_plc_connection);
    assert_eq!(cfg2.modbus_address_offset, -1);
    assert_eq!(cfg2.word_order, cfg.word_order);
    let ipv6 = Endpoint {
        ip: "::1".into(),
        ..value()
    }
    .apply(&cfg)
    .unwrap();
    assert_eq!(ipv6.address, "[::1]:1504");
    let plc = SimulatorConfig {
        mode: "plc".into(),
        allow_plc_connection: false,
        ..cfg
    };
    assert!(value().apply(&plc).is_ok()); // A read-only probe does not grant write/collection permissions.
    assert!(
        Endpoint {
            ip: "192.0.2.10".into(),
            ..value()
        }
        .apply(&cfg2)
        .is_ok()
    );
}
#[tokio::test]
async fn read_save_restart_stale_revision_and_unsafe_state() {
    let dir = Fixture::new();
    let state = dir.state().await;
    assert_eq!(
        simulator_config::locate(Some(&dir.0)).unwrap(),
        dir.0.join("as200_config.toml")
    );
    let original = get(&state).await.unwrap();
    assert_eq!(original.endpoint.as_ref().unwrap().ip, "127.0.0.1");
    let journal = std::fs::read(dir.0.join("as200_capture_journal.json")).unwrap();
    state.status.write().await.enabled = true;
    assert_eq!(
        save(&state, value(), &original.revision)
            .await
            .err()
            .unwrap(),
        BUSY
    );
    assert_eq!(
        std::fs::read_to_string(dir.0.join("as200_config.toml")).unwrap(),
        TEXT
    );
    state.status.write().await.enabled = false;
    for extra in [
        serde_json::json!({"pause":"CONFLICT"}),
        serde_json::json!({"last_total":1}),
        serde_json::json!({"ack":{"accepted":1,"binding":"","intent":null}}),
        serde_json::json!({"ack":{"binding":"old-endpoint","accepted":0,"intent":null}}),
        serde_json::json!({"ack":{"binding":"","accepted":0,"intent":{"target":1,"records":[],"submitted_at_ms":10}}}),
    ] {
        dir.journal(extra);
        assert_eq!(
            save(&state, value(), &original.revision)
                .await
                .err()
                .unwrap(),
            BUSY
        );
    }
    dir.journal(serde_json::json!({}));
    let external_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.0.join("as200_capture_journal.lock"))
        .unwrap();
    external_lock.try_lock().unwrap();
    assert_eq!(
        save(&state, value(), &original.revision)
            .await
            .err()
            .unwrap(),
        BUSY
    );
    drop(external_lock);
    let result = save(&state, value(), &original.revision).await.unwrap();
    assert!(result.restart_required);
    assert!(get(&state).await.unwrap().restart_required);
    assert_eq!(get(&state).await.unwrap().endpoint, Some(value()));
    assert!(
        save(
            &state,
            Endpoint {
                port: 1505,
                ..value()
            },
            &original.revision
        )
        .await
        .is_err()
    );
    assert_eq!(
        std::fs::read(dir.0.join("as200_capture_journal.json")).unwrap(),
        journal
    );
    let (loaded, _) = simulator_config::load(Some(&dir.0)).unwrap().unwrap();
    assert_eq!(loaded.address, "127.0.0.2:1504");
    assert_eq!(loaded.unit_id, 2);
    assert!(!loaded.enabled);
    assert!(
        !dir.0.read_dir().unwrap().any(|e| e
            .unwrap()
            .path()
            .extension()
            .is_some_and(|e| e == "tmp"))
    );
    let empty = WorkerState::default();
    empty.settings_ready.store(true, Ordering::SeqCst);
    assert!(get(&empty).await.unwrap().endpoint.is_none());
}

#[tokio::test]
async fn missing_or_unconfirmed_journal_never_allows_endpoint_change() {
    let dir = Fixture::new();
    let state = dir.state().await;
    let initial = get(&state).await.unwrap();
    dir.journal(serde_json::json!({"session_id":"another-session"}));
    assert_eq!(
        save(&state, value(), &initial.revision)
            .await
            .err()
            .unwrap(),
        BUSY
    );
    std::fs::remove_file(dir.0.join("as200_capture_journal.json")).unwrap();
    assert_eq!(
        save(&state, value(), &initial.revision)
            .await
            .err()
            .unwrap(),
        BUSY
    );
    assert_eq!(
        std::fs::read_to_string(dir.0.join("as200_config.toml")).unwrap(),
        TEXT
    );
    dir.journal(serde_json::json!({"version":1}));
    let before = std::fs::read(dir.0.join("as200_capture_journal.json")).unwrap();
    save(&state, value(), &initial.revision).await.unwrap();
    assert_eq!(
        std::fs::read(dir.0.join("as200_capture_journal.json")).unwrap(),
        before
    );
}

#[tokio::test]
async fn probe_is_single_fc03_validates_control_and_never_writes() {
    for valid in [true, false] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 12];
            stream.read_exact(&mut request).await.unwrap();
            assert_eq!(request[6], 1);
            assert_eq!(request[7], 3);
            assert_eq!(u16::from_be_bytes([request[8], request[9]]), 24999);
            assert_eq!(u16::from_be_bytes([request[10], request[11]]), 3);
            let mut words = vec![0u16; 3];
            words[0] = if valid { 1 } else { 99 };
            words[1] = 240;
            words[2] = 2;
            let mut response = vec![request[0], request[1], 0, 0, 0, 9, 1, 3, 6];
            for word in words {
                response.extend(word.to_be_bytes());
            }
            stream.write_all(&response).await.unwrap();
            let mut extra = [0u8; 12];
            assert_eq!(
                tokio::time::timeout(std::time::Duration::from_secs(1), stream.read(&mut extra))
                    .await
                    .unwrap()
                    .unwrap(),
                0
            );
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(30), listener.accept())
                    .await
                    .is_err()
            );
        });
        let mut cfg = simulator_config::parse(TEXT).unwrap();
        cfg.address = address.to_string();
        assert_eq!(probe(cfg).await.is_ok(), valid);
        server.await.unwrap();
    }
}
#[tokio::test]
async fn probe_has_total_timeout_and_rejects_open_tcp_without_modbus() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    });
    let mut cfg = simulator_config::parse(TEXT).unwrap();
    cfg.address = address.to_string();
    assert_eq!(probe(cfg).await.unwrap_err(), CONNECT);
    server.abort();
}

#[tokio::test]
async fn first_installation_persists_only_endpoint_and_never_creates_identity_or_journal() {
    let dir = Fixture::new();
    let state = WorkerState::default();
    state.settings_ready.store(true, Ordering::SeqCst);
    let initial = get(&state).await.unwrap();
    assert!(!initial.configured);
    assert!(initial.endpoint.is_none());
    let request = Endpoint {
        ip: "192.0.2.10".into(),
        port: 502,
        unit_id: 7,
    };
    let path = create_initial(&dir.0, &request).unwrap();
    let before = std::fs::read_to_string(&path).unwrap();
    let values: toml::Value = toml::from_str(&before).unwrap();
    for key in [
        "source_key",
        "confirmed_session_id",
    ] {
        assert!(values.get(key).is_none());
    }
    let cfg = simulator_config::parse(&before).unwrap();
    assert!(!cfg.enabled);
    assert!(!cfg.technical_setup_confirmed);
    assert!(!cfg.ack_enabled);
    assert!(!cfg.pc_heartbeat_enabled);
    assert!(cfg.source_key.is_empty());
    assert!(cfg.confirmed_session_id.is_none());
    assert!(cfg.validate().is_err());
    assert!(simulator_config::parse(&before.replace("enabled = false", "enabled = true")).is_err());
    assert!(create_initial(&dir.0, &value()).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    *state.settings_path.lock().await = Some(path.clone());
    let reopened = get(&state).await.unwrap();
    assert_eq!(reopened.endpoint, Some(request));
    assert!(!reopened.technical_ready);
    save(&state, value(), &reopened.revision).await.unwrap();
    assert_eq!(get(&state).await.unwrap().endpoint, Some(value()));
    assert!(!dir.0.join("as200_capture_journal.json").exists());
    assert!(!dir.0.join("as200_capture_journal.lock").exists());
    let other = Fixture::new();
    std::fs::write(
        other.0.join("as200_capture_journal.json"),
        "preserve existing history",
    )
    .unwrap();
    assert!(create_initial(&other.0, &value()).is_err());
    assert!(!other.0.join("as200_config.toml").exists());
}

/// Fake AS200 trả FC03 từ mảng register (D25000.., offset 0) cho đến khi client đóng socket.
fn serve_registers(listener: tokio::net::TcpListener, words: Vec<u16>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 12];
        while stream.read_exact(&mut request).await.is_ok() {
            assert_eq!(request[7], 3, "kiểm tra kết nối chỉ được dùng FC03");
            let base = usize::from(u16::from_be_bytes([request[8], request[9]])) - 25000;
            let count = usize::from(u16::from_be_bytes([request[10], request[11]]));
            let mut response = request[..7].to_vec();
            response[4..6].copy_from_slice(&((count * 2 + 3) as u16).to_be_bytes());
            response.extend([3, (count * 2) as u8]);
            for word in &words[base..base + count] {
                response.extend(word.to_be_bytes());
            }
            stream.write_all(&response).await.unwrap();
        }
    })
}
fn probe_config(address: String) -> SimulatorConfig {
    let mut cfg = simulator_config::parse(TEXT).unwrap();
    cfg.address = address;
    cfg.modbus_address_offset = 0;
    cfg.timeout_ms = 1000;
    cfg
}

#[tokio::test]
async fn test_connection_succeeds_when_registers_are_read_even_if_mapping_is_wrong() {
    // Header sai (0/0/0 thay vì 1/240/2): vẫn là kết nối thành công, kèm cảnh báo mapping.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let server = serve_registers(listener, vec![0; 500]);
    let warning = probe_link(probe_config(address.clone())).await.unwrap().unwrap();
    assert!(warning.contains("đọc được = 0/0/0, cần 1/240/2"), "{warning}");
    server.await.unwrap();
    // Bật thu + ACK vẫn bắt buộc mapping đúng.
    let listener = tokio::net::TcpListener::bind(address.as_str()).await.unwrap();
    let server = serve_registers(listener, vec![0; 500]);
    let error = probe(probe_config(address)).await.unwrap_err();
    assert!(error.starts_with("Đã kết nối và đọc được PLC"), "{error}");
    server.await.unwrap();
}

#[tokio::test]
async fn test_connection_without_warning_when_mapping_is_valid() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let mut words = vec![0u16; 500];
    words[0] = 1; words[1] = 240; words[2] = 2;
    let server = serve_registers(listener, words);
    assert_eq!(probe_link(probe_config(address)).await.unwrap(), None);
    server.await.unwrap();
}

#[tokio::test]
async fn test_connection_fails_only_when_plc_cannot_be_reached() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    drop(listener);
    let mut cfg = probe_config(address);
    cfg.timeout_ms = 300;
    assert_eq!(probe_link(cfg).await.unwrap_err(), CONNECT);
}
