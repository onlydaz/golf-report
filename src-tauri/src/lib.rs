mod commands;
pub mod database;
pub mod enums;
pub mod models;
mod plc;
pub mod repositories;
pub mod services;

use tauri::Manager;

#[cfg(not(debug_assertions))]
fn release_plc_directory(app_config_dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
    use std::{fs, fs::OpenOptions, io::Write, path::PathBuf};
    let root = std::env::var_os("ProgramData")
        .filter(|value| !value.is_empty())
        .ok_or("Windows không cung cấp đường dẫn ProgramData.")?;
    let dir = PathBuf::from(root).join("GolfReport");
    // Existing per-user PLC history must be reconciled; an empty shared directory
    // alone cannot establish that this is a new physical source.
    if !dir.join("as200_config.toml").exists() {
        if ["as200_capture_journal.json", "as200_capture_journal.lock", "local-runtime"]
            .iter().any(|name| dir.join(name).exists()) {
            return Err("Đã tìm thấy lịch sử PLC trong ProgramData nhưng thiếu cấu hình. Cần kỹ thuật đối chiếu trước khi thiết lập.".into());
        }
        if ["as200_config.toml", "as200_capture_journal.json", "as200_capture_journal.lock", "local-runtime"]
            .iter().any(|name| app_config_dir.join(name).exists()) {
            return Err("Đã tìm thấy lịch sử PLC trong AppData. Cần kỹ thuật đối chiếu trước khi thiết lập trong ProgramData.".into());
        }
    }
    fs::create_dir_all(&dir).map_err(|_| "Không thể tạo thư mục GolfReport trong ProgramData. Hãy kiểm tra bản cài đặt và quyền ghi.".to_string())?;
    let probe = dir.join(format!(".write-check-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()));
    let mut file = OpenOptions::new().write(true).create_new(true).open(&probe)
        .map_err(|_| "Không có quyền ghi vào thư mục GolfReport trong ProgramData. Hãy chạy lại bộ cài hoặc liên hệ kỹ thuật.".to_string())?;
    let result = file.write_all(b"check").and_then(|_| file.sync_all());
    drop(file);
    let cleanup = fs::remove_file(&probe);
    result.map_err(|_| "Không thể ghi dữ liệu PLC vào ProgramData.".to_string())?;
    cleanup.map_err(|_| "Không thể hoàn tất kiểm tra quyền ghi ProgramData.".to_string())?;
    Ok(dir)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(database::DatabaseState::default())
        .manage(plc::worker::WorkerState::default())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            let app_handle = app.handle().clone();
            let database_state = app.state::<database::DatabaseState>().inner().clone();
            let worker = app.state::<plc::worker::WorkerState>().inner().clone();
            let worker_database = database_state.clone();
            let app_config_dir = app.path().app_config_dir()?;
            #[cfg(debug_assertions)]
            let config_dir = Ok(app_config_dir);
            #[cfg(not(debug_assertions))]
            let config_dir = release_plc_directory(&app_config_dir);
            tauri::async_runtime::spawn(async move {
                match config_dir {
                    Ok(config_dir) => worker.launch(Some(config_dir), worker_database).await,
                    Err(error) => worker.report_runtime_initialization_failure(error).await,
                }
            });
            tauri::async_runtime::spawn(async move {
                database_state.initialize(&app_handle).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::measurement_command::check_database_connection,
            commands::measurement_command::get_database_status,
            commands::measurement_command::retry_database_connection,
            commands::measurement_command::get_measurements,
            commands::measurement_command::get_measurement_by_id,
            commands::measurement_command::get_measurement_statistics,
            commands::measurement_command::get_measurement_report,
            plc::worker::get_as200_worker_status,
            plc::worker::reconnect_as200_plc,
            plc::connection_settings::get_plc_connection_settings,
            plc::connection_settings::stop_plc_collection,
            plc::connection_settings::connect_plc_collection,
            plc::connection_settings::test_plc_connection,
            plc::connection_settings::update_plc_connection_settings,
            plc::connection_settings::confirm_initial_plc_setup,
            plc::worker::retry_as200_inbox,
            plc::worker::get_as200_retry_diagnostics,
            plc::worker::acknowledge_as200_buffer_gap
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
