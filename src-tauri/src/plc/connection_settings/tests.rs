use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn saving_an_unknown_endpoint_only_stages_settings_and_preserves_running_source() {
    let dir = Fixture::new();
    let state = dir.state().await;
    let file = dir.0.join("as200_config.toml");
    let (_, config) = read(&file).unwrap();
    let before = std::fs::read(&file).unwrap();
    let journal_path = config.journal_file(&file);
    let journal = std::fs::read(&journal_path).unwrap();
    {
        let mut status = state.status.write().await;
        status.enabled = true;
        status.connection = "CONNECTED".into();
        status.address = config.address.clone();
        status.unit_id = config.unit_id;
    }
    let value = RuntimeSettings {
        endpoint: Endpoint {
            ip: "127.0.0.2".into(),
            port: 1504,
            unit_id: 2,
        },
        parameters: RuntimeParameters::from_config(&config),
    };
    let saved = save_connection_draft(&state, value.clone(), &revision(TEXT))
        .await
        .unwrap();
    assert_eq!(saved.endpoint.as_ref(), Some(&value.endpoint));
    assert!(
        saved.parameters.read_only,
        "New targets must not inherit ACK permission"
    );
    assert_eq!(
        saved.active_endpoint.as_ref(),
        Some(&endpoint(&config).unwrap())
    );
    assert!(saved.worker_running);
    assert!(saved.verification_required);
    assert!(saved.restart_required);
    assert_eq!(std::fs::read(&file).unwrap(), before);
    assert_eq!(std::fs::read(&journal_path).unwrap(), journal);
    assert!(
        qc_profile_history(&file, &config, &value.endpoint)
            .unwrap()
            .is_none()
    );
    let draft = std::fs::read_to_string(file.with_extension("connection-draft")).unwrap();
    for field in [
        "source_key",
        "session_id",
        "journal_path",
        "accepted",
        "receipt",
        "ack_enabled",
    ] {
        assert!(
            !draft.contains(field),
            "draft must not carry source history or ACK: {field}"
        );
    }
    assert!(
        save_connection_draft(&state, value, &revision(TEXT))
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(file.with_extension("connection-draft")).unwrap(),
        draft
    );
    assert!(state.status.read().await.enabled);
}

#[tokio::test]
async fn connection_draft_is_invalidated_by_a_changed_live_profile_and_never_autostarts() {
    let dir = Fixture::new();
    let state = dir.state().await;
    let file = dir.0.join("as200_config.toml");
    let (_, config) = read(&file).unwrap();
    let mut parameters = RuntimeParameters::from_config(&config);
    parameters.poll_interval_ms = 1000;
    let value = RuntimeSettings {
        endpoint: endpoint(&config).unwrap(),
        parameters,
    };
    let saved = save_connection_draft(&state, value, &revision(TEXT))
        .await
        .unwrap();
    assert_eq!(saved.parameters.poll_interval_ms, 1000);
    assert!(!state.status.read().await.enabled);
    assert_eq!(
        read(&file).unwrap().1.poll_interval_ms,
        config.poll_interval_ms
    );
    let external = format!("{TEXT}# external edit\n");
    std::fs::write(&file, &external).unwrap();
    assert!(connection_draft(&file, &external).unwrap().is_none());
    let restored = get(&state).await.unwrap();
    assert_eq!(
        restored.parameters.poll_interval_ms,
        config.poll_interval_ms
    );
    assert_eq!(restored.revision, revision(&external));
    assert!(!state.status.read().await.enabled);
}

#[tokio::test]
async fn inspection_of_an_unregistered_source_is_fc03_only_and_does_not_grant_collection() {
    for valid in [true, false] {
        let dir = Fixture::new();
        let state = dir.state().await;
        let file = dir.0.join("as200_config.toml");
        let (_, current) = read(&file).unwrap();
        let journal_path = current.journal_file(&file);
        let journal = std::fs::read(&journal_path).unwrap();
        let config_bytes = std::fs::read(&file).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            assert_eq!(
                tcp.read(&mut [0; 1]).await.unwrap(),
                0,
                "TCP reachability must not send data"
            );
            drop(tcp);
            let mut words = vec![0; 500];
            words[0] = if valid { 1 } else { 99 };
            words[1] = 240;
            words[2] = 2;
            serve_registers(listener, words).await.unwrap();
        });
        let target = Endpoint {
            ip: "127.0.0.1".into(),
            port: address.port().into(),
            unit_id: 1,
        };
        let runtime = RuntimeSettings {
            endpoint: target.clone(),
            parameters: RuntimeParameters::from_config(&probe_config(address.to_string())),
        };
        let saved = save_connection_draft(&state, runtime, &revision(TEXT))
            .await
            .unwrap();
        assert_eq!(saved.endpoint.as_ref(), Some(&target));
        // Missing history is a collection decision, not a reason to skip the probe.
        assert!(
            qc_profile_history(&file, &current, &target)
                .unwrap()
                .is_none()
        );
        let reason = "Baseline chưa được thiết lập; chưa cấp ACK".to_string();
        let inspected = inspect_endpoint(probe_config(address.to_string())).await;
        assert!(inspected.tcp_connected);
        assert!(inspected.modbus_read_succeeded);
        assert_eq!(inspected.as200_valid, valid);
        let outcome = ConnectionOutcome::inspection_only(inspected, reason);
        assert!(!outcome.collection_started);
        assert!(
            outcome
                .block_reason
                .as_deref()
                .unwrap()
                .contains("Baseline")
        );
        assert_eq!(std::fs::read(&file).unwrap(), config_bytes);
        assert_eq!(std::fs::read(&journal_path).unwrap(), journal);
        assert!(!state.status.read().await.enabled);
        assert!(!dir.0.join("local-runtime/managed-sources").exists());
        server.await.unwrap();
    }
}

#[tokio::test]
async fn inspection_separates_tcp_only_from_modbus_and_refused_tcp() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut tcp, _) = listener.accept().await.unwrap();
        assert_eq!(tcp.read(&mut [0; 1]).await.unwrap(), 0);
        drop(tcp);
        let (_modbus, _) = listener.accept().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    });
    let mut config = probe_config(address.to_string());
    config.timeout_ms = 100;
    let result = inspect_endpoint(config.clone()).await;
    assert!(result.tcp_connected);
    assert!(!result.modbus_read_succeeded);
    assert!(!result.as200_valid);
    server.abort();
    let _ = server.await;
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    config.address = closed.local_addr().unwrap().to_string();
    drop(closed);
    let result = inspect_endpoint(config).await;
    assert!(!result.tcp_connected);
    assert!(!result.modbus_read_succeeded);
    assert!(!result.as200_valid);
}

#[test]
fn qc_requires_existing_verified_identity_and_keeps_ack_policy() {
    let config = SimulatorConfig {
        enabled: true,
        mode: "modbus".into(),
        technical_setup_confirmed: true,
        source_key: "saved-source".into(),
        confirmed_session_id: Some("saved-session".into()),
        journal_path: Some(PathBuf::from(
            "local-runtime/managed-sources/saved-source/capture.json",
        )),
        read_only: true,
        ack_enabled: false,
        ack_protocol: Some("as200".into()),
        allow_plc_connection: true,
        ..Default::default()
    };
    require_qc_profile(&config).unwrap();
    let dir = Fixture::new();
    let file = dir.0.join("as200_config.toml");
    std::fs::write(&file, "enabled=true\nmode='modbus'\ntechnical_setup_confirmed=true\naddress='127.0.0.1:1503'\nunit_id=1\nsource_key='saved-source'\nconfirmed_session_id='saved-session'\njournal_path='local-runtime/managed-sources/saved-source/capture.json'\nread_only=true\nack_enabled=false\nallow_plc_connection=true\nack_protocol='as200'\n").unwrap();
    let before = std::fs::read(&file).unwrap();
    assert!(
        qc_profile_history(&file, &config, &endpoint(&config).unwrap())
            .unwrap()
            .is_none()
    );
    let unknown = Endpoint {
        ip: "127.0.0.2".into(),
        port: 1503,
        unit_id: 1,
    };
    assert!(
        qc_profile_history(&file, &config, &unknown)
            .unwrap()
            .is_none()
    );
    let a = endpoint_source_key(&endpoint(&config).unwrap()).unwrap();
    assert_ne!(a, endpoint_source_key(&unknown).unwrap());
    assert_ne!(
        a,
        endpoint_source_key(&Endpoint {
            port: 1504,
            ..endpoint(&config).unwrap()
        })
        .unwrap()
    );
    assert_ne!(
        a,
        endpoint_source_key(&Endpoint {
            unit_id: 2,
            ..endpoint(&config).unwrap()
        })
        .unwrap()
    );
    assert_eq!(
        endpoint_source_key(&Endpoint {
            ip: "0:0:0:0:0:0:0:1".into(),
            port: 1503,
            unit_id: 1
        })
        .unwrap(),
        endpoint_source_key(&Endpoint {
            ip: "::1".into(),
            port: 1503,
            unit_id: 1
        })
        .unwrap()
    );
    assert_eq!(std::fs::read(&file).unwrap(), before);
    assert_eq!(
        std::fs::read_dir(&dir.0).unwrap().count(),
        1,
        "QC must not create registry/profile/Journal files for an unknown endpoint"
    );
    assert!(!config.ack_enabled);
    assert_eq!(
        config.confirmed_session_id.as_deref(),
        Some("saved-session")
    );
    for (missing, code) in [
        (
            SimulatorConfig {
                technical_setup_confirmed: false,
                ..config.clone()
            },
            "SOURCE_NOT_VERIFIED",
        ),
        (
            SimulatorConfig {
                source_key: String::new(),
                ..config.clone()
            },
            "SOURCE_ID_MISSING",
        ),
        (
            SimulatorConfig {
                confirmed_session_id: None,
                ..config.clone()
            },
            "SESSION_MISSING",
        ),
        (
            SimulatorConfig {
                confirmed_session_id: Some(" ".into()),
                ..config.clone()
            },
            "SESSION_MISSING",
        ),
        (
            SimulatorConfig {
                journal_path: None,
                ..config.clone()
            },
            "JOURNAL_MISSING",
        ),
        (
            SimulatorConfig {
                journal_path: Some(PathBuf::new()),
                ..config.clone()
            },
            "JOURNAL_MISSING",
        ),
        (
            SimulatorConfig {
                enabled: false,
                ..config.clone()
            },
            "SOURCE_DISABLED",
        ),
    ] {
        assert!(require_qc_profile(&missing).unwrap_err().contains(code));
    }
    let invalid_map = SimulatorConfig {
        record_size: 239,
        ..config
    };
    assert!(require_qc_profile(&invalid_map).is_err());
}

#[test]
fn new_source_requires_external_identity_and_map_confirmation_without_default_ack() {
    let mut verification: SourceVerification = serde_json::from_value(serde_json::json!({
        "physicalIdentity": " SERIAL-TEST-1 ", "newSourceConfirmed": true, "registerMapConfirmed": true
    })).unwrap();
    assert_eq!(verification.validate().unwrap(), "SERIAL-TEST-1");
    assert!(!verification.ack_write_approved);
    verification.new_source_confirmed = false;
    assert!(verification.validate().is_err());
    verification.new_source_confirmed = true;
    verification.register_map_confirmed = false;
    assert!(verification.validate().is_err());
    verification.register_map_confirmed = true;
    for identity in [
        "".to_string(),
        "  ".to_string(),
        "bad\nserial".to_string(),
        "x".repeat(201),
    ] {
        verification.physical_identity = identity;
        assert!(verification.validate().is_err());
    }
}

#[tokio::test]
#[ignore = "Isolated loopback PostgreSQL only: AS200_SOURCE_SWITCH_TEST_PORT; TEMP tables and FC03 mocks"]
async fn automatic_endpoint_source_is_read_only_separate_and_recoverable() {
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    let port: u16 = std::env::var("AS200_SOURCE_SWITCH_TEST_PORT")
        .unwrap()
        .parse()
        .unwrap();
    assert!(port >= 1024);
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(
            PgConnectOptions::new()
                .host("127.0.0.1")
                .port(port)
                .username("postgres")
                .database("golf_report_point_test"),
        )
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TEMP TABLE plc_sources(id bigserial PRIMARY KEY,source_key text UNIQUE,description text); CREATE TEMP TABLE plc_source_registry(source_id bigint,endpoint_ip inet,endpoint_port integer,unit_id integer,as200_verified boolean,ack_write_approved boolean); CREATE TEMP TABLE plc_inbox(source_id bigint); CREATE TEMP TABLE measurements(source_id bigint);").execute(&pool).await.unwrap();
    let dir = Fixture::new();
    let file = dir.0.join("as200_config.toml");
    std::fs::write(&file, TEXT).unwrap();
    let old_journal = dir.0.join("old.json");
    std::fs::write(&old_journal, "keep old receipts and ACK intent").unwrap();
    let mut identities = Vec::new();
    for total in [0_u16, 0, 1] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..if total == 0 { 2 } else { 1 } {
                let (mut socket, _) = listener.accept().await.unwrap();
                for _ in 0..if total == 0 { 9 } else { 1 } {
                    let mut request = [0; 12];
                    socket.read_exact(&mut request).await.unwrap();
                    assert_eq!(request[7], 3, "Automatic sources never write ACK/heartbeat");
                    let offset = u16::from_be_bytes([request[8], request[9]]) as usize - 25000;
                    let count = u16::from_be_bytes([request[10], request[11]]) as usize;
                    let mut words = vec![0u16; 500];
                    words[0] = 1;
                    words[1] = 240;
                    words[2] = 2;
                    words[4] = total;
                    let mut response = request[..7].to_vec();
                    response[4..6].copy_from_slice(&((count * 2 + 3) as u16).to_be_bytes());
                    response.extend([3, (count * 2) as u8]);
                    for word in &words[offset..offset + count] {
                        response.extend(word.to_be_bytes());
                    }
                    socket.write_all(&response).await.unwrap();
                }
            }
        });
        let target = Endpoint {
            ip: "127.0.0.1".into(),
            port: address.port().into(),
            unit_id: 1,
        };
        let config = SimulatorConfig {
            address: address.to_string(),
            mode: "modbus".into(),
            source_key: "outgoing".into(),
            confirmed_session_id: Some("outgoing-session".into()),
            journal_path: Some(old_journal.clone()),
            ack_enabled: true,
            ack_protocol: Some("as200".into()),
            ..Default::default()
        };
        let prepared = prepare_managed_runtime(&file, TEXT, &config, &target, &pool, None).await;
        server.await.unwrap();
        if total != 0 {
            assert!(prepared.is_err());
            assert!(
                !dir.0
                    .join("local-runtime/managed-sources")
                    .join(endpoint_source_key(&target).unwrap())
                    .exists()
            );
            continue;
        }
        let (after, next) = prepared.unwrap();
        assert!(
            next.read_only
                && !next.ack_enabled
                && !next.pc_heartbeat_enabled
                && !next.allow_plc_heartbeat_write
        );
        assert_eq!(next.source_key, endpoint_source_key(&target).unwrap());
        assert_ne!(next.source_key, config.source_key);
        assert_ne!(next.confirmed_session_id, config.confirmed_session_id);
        let journal = std::fs::read(next.journal_path.as_ref().unwrap()).unwrap();
        let captured: serde_json::Value = serde_json::from_slice(&journal).unwrap();
        assert_eq!(captured["version"], 4);
        assert_eq!(captured["published_ack"]["accepted"], 0);
        assert!(captured["published_ack"]["intent"].is_null());
        assert_eq!(captured["deliveries"].as_array().unwrap().len(), 0);
        let restored = qc_profile_history(&file, &simulator_config::parse(TEXT).unwrap(), &target)
            .unwrap()
            .unwrap();
        assert_eq!(restored.config.source_key, next.source_key);
        assert_eq!(
            restored.config.confirmed_session_id,
            next.confirmed_session_id
        );
        assert_eq!(
            std::fs::read(restored.config.journal_path.unwrap()).unwrap(),
            journal
        );
        verify_registered_source(&pool, &next).await.unwrap();
        let mut unauthorized = next.clone();
        unauthorized.read_only = false;
        unauthorized.ack_enabled = true;
        assert!(
            verify_registered_source(&pool, &unauthorized)
                .await
                .unwrap_err()
                .contains("ACK_NOT_APPROVED")
        );
        assert!(simulator_config::parse(&after).unwrap().read_only);
        identities.push((next.source_key, next.journal_path));
    }
    assert_ne!(identities[0], identities[1]);
    assert_eq!(
        std::fs::read_to_string(&old_journal).unwrap(),
        "keep old receipts and ACK intent"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), TEXT);
    let grants: i64 =
        sqlx::query_scalar("SELECT count(*) FROM plc_source_registry WHERE ack_write_approved")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(grants, 0);
}

#[tokio::test]
#[ignore = "Isolated loopback PostgreSQL only: AS200_SOURCE_SWITCH_TEST_PORT; TEMP tables and mock FC03"]
async fn source_registry_and_new_journal_are_separate_without_inherited_ack() {
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    let port: u16 = std::env::var("AS200_SOURCE_SWITCH_TEST_PORT")
        .unwrap()
        .parse()
        .unwrap();
    assert!(port >= 1024);
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(
            PgConnectOptions::new()
                .host("127.0.0.1")
                .port(port)
                .username("postgres")
                .database("golf_report_point_test"),
        )
        .await
        .unwrap();
    // TEMP tables shadow persistent names on this single test connection.
    sqlx::raw_sql(r#"
        CREATE TEMP TABLE plc_sources(id bigserial PRIMARY KEY,source_key text UNIQUE,description text);
        CREATE TEMP TABLE plc_source_registry(source_id bigint PRIMARY KEY REFERENCES plc_sources(id),
            physical_identity text UNIQUE,endpoint_ip inet,endpoint_port integer,unit_id integer,
            session_id text,as200_verified boolean,ack_write_approved boolean,verified_at timestamptz,
            UNIQUE(endpoint_ip,endpoint_port,unit_id));
        CREATE TEMP TABLE plc_inbox(source_id bigint);
        CREATE TEMP TABLE measurements(source_id bigint);
        INSERT INTO plc_sources(source_key,description) VALUES('old-simulator','do not overwrite');
    "#).execute(&pool).await.unwrap();
    let verification = SourceVerification {
        physical_identity: "TEST-SERIAL-NEW".into(),
        new_source_confirmed: true,
        register_map_confirmed: true,
        ack_write_approved: false,
    };
    let endpoint = Endpoint {
        ip: "127.0.0.1".into(),
        port: 1503,
        unit_id: 1,
    };
    let first = ensure_managed_source(&pool, &endpoint, &verification)
        .await
        .unwrap();
    let mut registered_config = SimulatorConfig {
        source_key: first.0.clone(),
        mode: "modbus".into(),
        address: "127.0.0.1:1503".into(),
        read_only: true,
        ..Default::default()
    };
    verify_registered_source(&pool, &registered_config)
        .await
        .unwrap();
    registered_config.read_only = false;
    assert!(
        verify_registered_source(&pool, &registered_config)
            .await
            .is_err()
    );
    registered_config.read_only = true;
    registered_config.address = "127.0.0.1:1504".into();
    assert!(
        verify_registered_source(&pool, &registered_config)
            .await
            .is_err()
    );
    assert_ne!(first.0, "old-simulator");
    assert_eq!(
        first,
        ensure_managed_source(&pool, &endpoint, &verification)
            .await
            .unwrap()
    ); // Retry only an unused registration.
    let other = SourceVerification {
        physical_identity: "OTHER-DEVICE".into(),
        ..verification
    };
    assert!(
        ensure_managed_source(&pool, &endpoint, &other)
            .await
            .is_err()
    );
    assert!(
        ensure_managed_source(
            &pool,
            &Endpoint {
                port: 1504,
                ..endpoint.clone()
            },
            &SourceVerification {
                physical_identity: "TEST-SERIAL-NEW".into(),
                ..other
            }
        )
        .await
        .is_err()
    );
    let approval = SourceVerification {
        physical_identity: "TEST-SERIAL-NEW".into(),
        new_source_confirmed: true,
        register_map_confirmed: true,
        ack_write_approved: true,
    };
    assert!(
        ensure_managed_source(&pool, &endpoint, &approval)
            .await
            .is_err()
    ); // Cannot upgrade a saved registry grant.
    sqlx::query(
        "INSERT INTO measurements(source_id) SELECT id FROM plc_sources WHERE source_key=$1",
    )
    .bind(&first.0)
    .execute(&pool)
    .await
    .unwrap();
    let verification = SourceVerification {
        ack_write_approved: false,
        ..approval
    };
    assert!(
        ensure_managed_source(&pool, &endpoint, &verification)
            .await
            .is_err()
    ); // Missing endpoint profile cannot become a new epoch.

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for _ in 0..3 {
            let (mut socket, _) = listener.accept().await.unwrap();
            for _ in 0..9 {
                let mut request = [0; 12];
                socket.read_exact(&mut request).await.unwrap();
                assert_eq!(
                    request[7], 3,
                    "No ACK/heartbeat write while enrolling a source"
                );
                let offset = u16::from_be_bytes([request[8], request[9]]) as usize - 25000;
                let count = u16::from_be_bytes([request[10], request[11]]) as usize;
                let mut words = vec![0_u16; 500];
                words[0] = 1;
                words[1] = 240;
                words[2] = 2;
                let mut response = request[..7].to_vec();
                response[4..6].copy_from_slice(&((count * 2 + 3) as u16).to_be_bytes());
                response.extend([3, (count * 2) as u8]);
                for word in &words[offset..offset + count] {
                    response.extend(word.to_be_bytes());
                }
                socket.write_all(&response).await.unwrap();
            }
        }
    });
    let dir = Fixture::new();
    let file = dir.0.join("as200_config.toml");
    std::fs::write(&file, TEXT).unwrap();
    let old_journal = dir.0.join("old-capture.json");
    std::fs::write(&old_journal, "preserve old journal, intent and checkpoint").unwrap();
    let config = SimulatorConfig {
        mode: "modbus".into(),
        address: address.to_string(),
        source_key: "old-simulator".into(),
        confirmed_session_id: Some("old-session".into()),
        journal_path: Some(old_journal.clone()),
        plc_heartbeat_timeout_ms: Some(5000),
        ..Default::default()
    };
    let target = Endpoint {
        ip: "127.0.0.1".into(),
        port: address.port() as u32,
        unit_id: 1,
    };
    let verification = SourceVerification {
        physical_identity: "MOCK-SERIAL-ONLY".into(),
        ..verification
    };
    let (after, next) =
        prepare_managed_runtime(&file, TEXT, &config, &target, &pool, Some(&verification))
            .await
            .unwrap();
    let new_journal_before = std::fs::read(next.journal_path.as_ref().unwrap()).unwrap();
    let repeated =
        prepare_managed_runtime(&file, TEXT, &config, &target, &pool, Some(&verification))
            .await
            .unwrap_err();
    assert!(repeated.contains("Journal riêng"));
    assert_eq!(
        std::fs::read(next.journal_path.as_ref().unwrap()).unwrap(),
        new_journal_before
    );
    server.await.unwrap();
    assert_ne!(next.source_key, config.source_key);
    assert_ne!(next.confirmed_session_id, config.confirmed_session_id);
    assert!(next.read_only && !next.ack_enabled && !next.pc_heartbeat_enabled);
    let parsed = simulator_config::parse(&after).unwrap();
    assert!(parsed.read_only && !parsed.ack_enabled);
    let journal: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(next.journal_path.unwrap()).unwrap())
            .unwrap();
    assert_eq!(journal["version"], 4);
    assert_eq!(journal["last_total"], 0);
    assert_eq!(journal["published_ack"]["accepted"], 0);
    assert!(journal["published_ack"]["intent"].is_null());
    assert!(journal["deliveries"].as_array().unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(old_journal).unwrap(),
        "preserve old journal, intent and checkpoint"
    );
    assert_eq!(std::fs::read_to_string(file).unwrap(), TEXT); // Preparation never replaces the active config.
    let old: String =
        sqlx::query_scalar("SELECT description FROM plc_sources WHERE source_key='old-simulator'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(old, "do not overwrite");
}

fn empty_new_snapshot() -> super::super::as200::reader::PublishedRecordSnapshot {
    use super::super::as200::{WordOrder, decode, reader::PublishedRecordSnapshot};
    let mut control = vec![0_u16; 20];
    control[0] = 1;
    control[1] = 240;
    control[2] = 2;
    PublishedRecordSnapshot {
        control: decode::control(&control, WordOrder::LowWordFirst).unwrap(),
        buffer: None,
        diagnostics: Vec::new(),
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
            _ => report.control.total_count = -1,
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
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    let profile = |address: &str, source: &str, journal: &Path| {
        format!(
            "mode='modbus'\nenabled=true\ntechnical_setup_confirmed=true\naddress='{address}'\nunit_id=1\nword_order='LOW'\nmodbus_address_offset=0\nplc_heartbeat_timeout_ms=5000\nsource_key='{source}'\nconfirmed_session_id='{source}-session'\njournal_path='{}'\n",
            journal.display()
        )
    };
    let a = profile("192.168.1.24:1504", "first", &first.join("capture.json"));
    let b = profile("127.0.0.1:1503", "second", &second.join("capture.json"));
    std::fs::write(&file, &b).unwrap();
    std::fs::write(second.join("previous-config.toml"), &a).unwrap();
    let requested = Endpoint {
        ip: "192.168.1.24".into(),
        port: 1504,
        unit_id: 1,
    };
    let historical = previous_endpoint(&file, &requested).unwrap().unwrap();
    assert_eq!(historical.config.source_key, "first");
    assert_eq!(
        historical.config.confirmed_session_id.as_deref(),
        Some("first-session")
    );
    assert_eq!(
        historical.config.journal_path.unwrap(),
        first.canonicalize().unwrap().join("capture.json")
    );
    assert!(
        previous_endpoint(
            &file,
            &Endpoint {
                ip: "127.0.0.2".into(),
                port: 1504,
                unit_id: 1
            }
        )
        .unwrap()
        .is_none()
    );
    retain_current_profile(&file, &b, &simulator_config::parse(&b).unwrap()).unwrap();
    std::fs::write(&file, &a).unwrap();
    let returning = previous_endpoint(
        &file,
        &Endpoint {
            ip: "127.0.0.1".into(),
            port: 1503,
            unit_id: 1,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(returning.config.source_key, "second");
    assert_eq!(
        std::fs::read_to_string(second.join("target-config.toml")).unwrap(),
        b
    );
    std::fs::write(&file, &b).unwrap();
    std::fs::remove_file(second.join("previous-config.toml")).unwrap();
    assert!(previous_endpoint(&file, &requested).is_err());
}

#[test]
fn endpoint_links_revisit_three_profiles_and_never_guess_between_legacy_identities() {
    let dir = Fixture::new();
    let file = dir.0.join("as200_config.toml");
    let mut profiles = Vec::new();
    for (name, port) in [
        ("one", 1505),
        ("two", 1506),
        ("three", 1507),
        ("legacy-one", 1505),
    ] {
        let folder = dir.0.join("local-runtime/managed-sources").join(name);
        std::fs::create_dir_all(&folder).unwrap();
        let text = format!(
            "mode='modbus'\nenabled=true\ntechnical_setup_confirmed=true\naddress='127.0.0.1:{port}'\nunit_id=1\nword_order='LOW'\nmodbus_address_offset=0\nplc_heartbeat_timeout_ms=5000\nsource_key='{name}'\nconfirmed_session_id='{name}-session'\njournal_path='{}'\n",
            folder.join("capture.json").display()
        );
        std::fs::write(folder.join("target-config.toml"), &text).unwrap();
        profiles.push((text.clone(), simulator_config::parse(&text).unwrap()));
    }
    for index in [0_usize, 1, 2, 0] {
        let (text, config) = &profiles[index];
        std::fs::write(&file, text).unwrap();
        remember_endpoint(&file, text, config).unwrap();
        let selected = previous_endpoint(&file, &endpoint(config).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(selected.config.source_key, config.source_key);
        assert_eq!(
            selected.config.confirmed_session_id,
            config.confirmed_session_id
        );
        assert_eq!(
            selected.config.journal_path,
            Some(profile_journal(&file, config).unwrap())
        );
    }
    let one = endpoint(&profiles[0].1).unwrap();
    std::fs::remove_file(endpoint_link(&file, &one).unwrap()).unwrap();
    assert_eq!(
        previous_endpoint(&file, &one).err().unwrap(),
        "HISTORY_AMBIGUOUS"
    );
}

#[tokio::test]
async fn controlled_baseline_reads_full_snapshot_using_only_fc03() {
    for total in [0_u16, 1_u16] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            for _ in 0..if total == 0 { 9 } else { 1 } {
                let mut request = [0; 12];
                socket.read_exact(&mut request).await.unwrap();
                assert_eq!(request[7], 3, "No Modbus write allowed during bootstrap");
                let offset = u16::from_be_bytes([request[8], request[9]]) as usize - 25000;
                let count = u16::from_be_bytes([request[10], request[11]]) as usize;
                let mut words = vec![0_u16; 500];
                words[0] = 1;
                words[1] = 240;
                words[2] = 2;
                words[4] = total;
                let mut response = request[..7].to_vec();
                response[4..6].copy_from_slice(&((count * 2 + 3) as u16).to_be_bytes());
                response.push(3);
                response.push((count * 2) as u8);
                for word in &words[offset..offset + count] {
                    response.extend_from_slice(&word.to_be_bytes());
                }
                socket.write_all(&response).await.unwrap();
            }
        });
        let config = SimulatorConfig {
            address,
            ..Default::default()
        };
        let result = read_new_baseline(&config).await;
        assert_eq!(result.is_ok(), total == 0);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn separate_journal_uses_engine_initialization_and_never_reuses_an_existing_path() {
    let dir = Fixture::new();
    let path = dir.0.join("new-source/capture.json");
    let config = SimulatorConfig {
        enabled: true,
        mode: "modbus".into(),
        technical_setup_confirmed: true,
        source_key: "controlled-new-source".into(),
        confirmed_session_id: Some("controlled-new-session".into()),
        journal_path: Some(path.clone()),
        ack_enabled: true,
        ack_protocol: Some("as200".into()),
        allow_plc_connection: true,
        ..Default::default()
    };
    WorkerState::initialize_separate_journal(config.clone(), path.clone())
        .await
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let journal: serde_json::Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(journal["version"], 4);
    assert_eq!(journal["source_key"], config.source_key);
    assert!(journal["published_ack"]["intent"].is_null());
    assert_eq!(journal["published_ack"]["accepted"], 0);
    assert_eq!(journal["last_total"], 0);
    assert!(
        WorkerState::initialize_separate_journal(config, path.clone())
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[tokio::test]
async fn interrupted_transition_blocks_autostart_and_legacy_endpoint_save() {
    let dir = Fixture::new();
    let state = dir.state().await;
    let file = dir.0.join("as200_config.toml");
    std::fs::write(file.with_extension("connection-pending"), TEXT).unwrap();
    state
        .clone()
        .launch(
            Some(dir.0.clone()),
            crate::database::DatabaseState::default(),
        )
        .await;
    assert_eq!(state.status.read().await.state, "STOPPED");
    assert!(!state.status.read().await.enabled);
    let result = save(
        &state,
        Endpoint {
            ip: "127.0.0.1".into(),
            port: 1504,
            unit_id: 1,
        },
        &revision(TEXT),
    )
    .await;
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
        let config = simulator_config::parse(TEXT).unwrap();
        let binding = format!(
            "published-ack-v1|{}|{}|{}|{}|{:?}|{}|{:?}|as200",
            config.mode,
            config.address,
            config.unit_id,
            config.source_key,
            config.confirmed_session_id,
            config.modbus_address_offset,
            config.word_order
        );
        let mut value = serde_json::json!({"version":4,"source_key":"settings-fixture","session_id":"confirmed-test-session","last_total":null,"pause":null,"warnings":[],"deliveries":[],
            "published_ack": {"binding": binding, "accepted": 0, "intent": null, "confirmed_receipt": null}});
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
    for version in [1, 2, 3] {
        dir.journal(serde_json::json!({"version":version}));
        let before = std::fs::read(dir.0.join("as200_capture_journal.json")).unwrap();
        assert_eq!(
            save(&state, value(), &initial.revision)
                .await
                .err()
                .expect("Legacy Journal must block endpoint changes"),
            BUSY
        );
        assert_eq!(
            std::fs::read(dir.0.join("as200_capture_journal.json")).unwrap(),
            before
        );
        assert_eq!(
            std::fs::read_to_string(dir.0.join("as200_config.toml")).unwrap(),
            TEXT
        );
    }
}

#[tokio::test]
async fn probe_reads_stable_control_block_using_only_fc03_and_never_writes() {
    for valid in [true, false] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            // DataReady=0: a valid published snapshot reads the complete
            // control block twice; a bad format is rejected on the first read.
            for _ in 0..if valid { 2 } else { 1 } {
                let mut request = [0; 12];
                stream.read_exact(&mut request).await.unwrap();
                assert_eq!(request[6], 1);
                assert_eq!(request[7], 3);
                assert_eq!(u16::from_be_bytes([request[8], request[9]]), 24999);
                assert_eq!(u16::from_be_bytes([request[10], request[11]]), 20);
                let mut words = [0u16; 20];
                words[0] = if valid { 1 } else { 99 };
                words[1] = 240;
                words[2] = 2;
                // Remaining control fields, including Count/Ready/Done, are 0.
                let mut response = request[..7].to_vec();
                response[4..6].copy_from_slice(&43u16.to_be_bytes());
                response.extend([3, 40]);
                for word in words {
                    response.extend(word.to_be_bytes());
                }
                stream.write_all(&response).await.unwrap();
            }
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
        let result = probe(cfg).await;
        if valid {
            result.unwrap();
        } else {
            let error = result.unwrap_err();
            assert!(
                error.contains("FormatVersion/RecordSize/BufferCount"),
                "{error}"
            );
        }
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
    for key in ["source_key", "confirmed_session_id"] {
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
fn serve_registers(
    listener: tokio::net::TcpListener,
    words: Vec<u16>,
) -> tokio::task::JoinHandle<()> {
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
    let warning = probe_link(probe_config(address.clone()))
        .await
        .unwrap()
        .unwrap();
    assert!(
        warning.contains("đọc được = 0/0/0, cần 1/240/2"),
        "{warning}"
    );
    server.await.unwrap();
    // Bật thu + ACK vẫn bắt buộc mapping đúng.
    let listener = tokio::net::TcpListener::bind(address.as_str())
        .await
        .unwrap();
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
    words[0] = 1;
    words[1] = 240;
    words[2] = 2;
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

#[tokio::test]
async fn runtime_settings_roundtrip_keeps_identity_and_journal_and_rejects_unverified_changes() {
    let dir = Fixture::new();
    let state = dir.state().await;
    let original = get(&state).await.unwrap();
    let journal = std::fs::read(dir.0.join("as200_capture_journal.json")).unwrap();
    let mut parameters = original.parameters.clone();
    parameters.poll_interval_ms = 750;
    parameters.timeout_ms = 3000;
    parameters.heartbeat_timeout_ms = Some(9000);
    parameters.retry_interval_ms = 2500;
    parameters.auto_reconnect = false;
    let request = || RuntimeSettings {
        endpoint: original.endpoint.clone().unwrap(),
        parameters: parameters.clone(),
    };
    state.status.write().await.enabled = true; // Saving does not restart or stop the active Worker.
    let saved = save_runtime(&state, request(), &original.revision)
        .await
        .unwrap();
    assert!(state.status.read().await.enabled);
    assert!(saved.restart_required);
    assert_eq!(saved.parameters, parameters);
    assert!(
        save_runtime(&state, request(), &original.revision)
            .await
            .is_err()
    );
    state.status.write().await.enabled = false;
    let (text, loaded) = read(&dir.0.join("as200_config.toml")).unwrap();
    assert_eq!(
        loaded.reader().timeout,
        std::time::Duration::from_millis(3000)
    );
    assert_eq!(
        loaded.source_key,
        simulator_config::parse(TEXT).unwrap().source_key
    );
    assert_eq!(
        loaded.confirmed_session_id,
        simulator_config::parse(TEXT).unwrap().confirmed_session_id
    );
    assert_eq!(
        std::fs::read(dir.0.join("as200_capture_journal.json")).unwrap(),
        journal
    );
    for (count, size) in [(3, 240), (2, 256), (2, 300)] {
        let mut rejected = parameters.clone();
        rejected.buffer_count = count;
        rejected.record_size = size;
        assert!(
            save_runtime(
                &state,
                RuntimeSettings {
                    endpoint: original.endpoint.clone().unwrap(),
                    parameters: rejected
                },
                &saved.revision
            )
            .await
            .is_err()
        );
    }
    for changed in 0..3 {
        let mut rejected = parameters.clone();
        match changed {
            0 => rejected.control_start = 26000,
            1 => rejected.word_order = StoredWordOrder::Low,
            _ => rejected.modbus_address_offset = 0,
        }
        assert!(
            save_runtime(
                &state,
                RuntimeSettings {
                    endpoint: original.endpoint.clone().unwrap(),
                    parameters: rejected
                },
                &saved.revision
            )
            .await
            .is_err()
        );
    }
    assert_eq!(
        std::fs::read_to_string(dir.0.join("as200_config.toml")).unwrap(),
        text
    );
    assert_eq!(
        std::fs::read(dir.0.join("as200_capture_journal.json")).unwrap(),
        journal
    );
}
#[test]
fn runtime_parameters_reject_fake_options_and_invalid_timing() {
    let c = SimulatorConfig::default();
    let good = RuntimeParameters::from_config(&c);
    for field in 0..8 {
        let mut p = good.clone();
        match field {
            0 => p.start_with_windows = true,
            1 => p.sku_table_start = Some(1000),
            2 => p.poll_interval_ms = 0,
            3 => p.poll_interval_ms = 49,
            4 => p.timeout_ms = 0,
            5 => p.heartbeat_timeout_ms = Some(1000),
            6 => p.retry_interval_ms = 0,
            _ => p.layout_profile = "unknown".into(),
        }
        assert!(p.apply(&c).is_err());
    }
    let mut p = good;
    p.read_only = true;
    p.control_start = 26000;
    let next = p.apply(&c).unwrap();
    assert!(!next.ack_enabled);
    assert_eq!(next.reader().control_start, 26000);
}

#[tokio::test]
async fn fresh_profile_can_save_relocated_read_only_parameters_without_creating_history() {
    let dir = Fixture::new();
    let file = create_initial(&dir.0, &value()).unwrap();
    let state = WorkerState::default();
    state.settings_ready.store(true, Ordering::SeqCst);
    *state.settings_path.lock().await = Some(file.clone());
    let current = get(&state).await.unwrap();
    let mut parameters = current.parameters.clone();
    parameters.control_start = 26000;
    parameters.word_order = StoredWordOrder::High;
    parameters.modbus_address_offset = -1;
    parameters.read_only = true;
    let saved = save_runtime(
        &state,
        RuntimeSettings {
            endpoint: value(),
            parameters: parameters.clone(),
        },
        &current.revision,
    )
    .await
    .unwrap();
    assert_eq!(saved.parameters, parameters);
    let (config, _) = simulator_config::load(Some(&dir.0)).unwrap().unwrap();
    assert_eq!(config.reader().control_start, 26000);
    assert_eq!(
        config.reader().word_order,
        super::super::as200::WordOrder::HighWordFirst
    );
    assert_eq!(config.reader().modbus_address_offset, -1);
    assert!(!config.ack_enabled && !config.enabled && !config.technical_setup_confirmed);
    assert!(config.source_key.is_empty() && config.confirmed_session_id.is_none());
    assert!(!dir.0.join("local-runtime").exists());
    assert!(!config.journal_file(&file).exists());
    assert!(saved.verification_required);
}
