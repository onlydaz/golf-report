mod commands;
pub mod database;
pub mod enums;
pub mod models;
mod plc;
pub use plc::as200;
pub mod repositories;
pub mod services;

use tauri::Manager;

pub(crate) fn application_directory() -> Result<std::path::PathBuf, String> {
    #[cfg(debug_assertions)]
    {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(std::path::Path::to_path_buf)
            .ok_or_else(|| "Không xác định được thư mục project của Phần Mềm Thu Thập Dữ Liệu Độ Cong Gậy Golf.".into())
    }
    #[cfg(not(debug_assertions))]
    {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
            .ok_or_else(|| "Không xác định được thư mục cài đặt Phần Mềm Thu Thập Dữ Liệu Độ Cong Gậy Golf.".into())
    }
}

pub(crate) fn check_plc_directory(dir: &std::path::Path) -> Result<(), String> {
    use std::{fs, fs::OpenOptions, io::Write};
    fn probe(dir: &std::path::Path) -> Result<(), String> {
        let first = dir.join(format!(
            ".golf-write-check-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let second = first.with_extension("renamed");
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&first)?;
            file.write_all(b"check")?;
            file.sync_all()?;
            drop(file);
            fs::rename(&first, &second)?;
            fs::remove_file(&second)
        })();
        if first.exists() {
            let _ = fs::remove_file(&first);
        }
        if second.exists() {
            let _ = fs::remove_file(&second);
        }
        result.map_err(|_: std::io::Error| format!("Không thể ghi/sync/đổi tên dữ liệu trong {}. Chưa bật thu hoặc ACK. Hãy kiểm tra quyền ghi thư mục ứng dụng.", dir.display()))
    }
    fn inspect(path: &std::path::Path) -> Result<(), String> {
        let metadata = fs::symlink_metadata(path)
            .map_err(|_| format!("Không đọc được thư mục runtime: {}", path.display()))?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(
                    "Runtime PLC chứa reparse point; cần kỹ thuật đối chiếu đường dẫn.".into(),
                );
            }
        }
        if metadata.file_type().is_symlink() {
            return Err("Runtime PLC chứa symlink; cần kỹ thuật đối chiếu đường dẫn.".into());
        }
        if metadata.is_dir() {
            probe(path)?;
            for entry in fs::read_dir(path).map_err(|_| "Không đọc được runtime PLC.")? {
                inspect(&entry.map_err(|_| "Không đọc được runtime PLC.")?.path())?;
            }
        } else if path.file_name().is_some_and(|name| name == "capture.json") {
            fs::File::open(path)
                .map_err(|_| "Không đọc được bằng chứng Journal PLC cũ để đối chiếu.")?;
        }
        Ok(())
    }
    probe(dir)?;
    let file = dir.join("as200_config.toml");
    let runtime = dir.join("local-runtime");
    if !file.exists()
        && (runtime.exists()
            || dir.join("as200_capture_journal.json").exists()
            || dir.join("as200_capture_journal.lock").exists())
    {
        // An empty installer-created local-runtime is allowed on a clean installation.
        let has_history = !runtime.is_dir()
            || fs::read_dir(&runtime)
                .map_err(|_| "Không đọc được local-runtime.")?
                .next()
                .is_some();
        if has_history
            || dir.join("as200_capture_journal.json").exists()
            || dir.join("as200_capture_journal.lock").exists()
        {
            return Err("Đã có lịch sử PLC nhưng thiếu as200_config.toml cạnh ứng dụng. Cần kỹ thuật đối chiếu; không tự tạo phiên hoặc cấu hình thay thế.".into());
        }
    }
    if file.exists() {
        inspect(&file)?;
        OpenOptions::new().write(true).open(&file).map_err(
            |_| "Không có quyền cập nhật as200_config.toml cạnh ứng dụng; chưa bật thu hoặc ACK.",
        )?;
        if let Some((config, journal)) = plc::simulator_config::load(Some(dir))? {
            if config.enabled && config.journal_path.is_none() {
                return Err("Cấu hình PLC đang bật nhưng thiếu đường dẫn Journal trong local-runtime; cần kỹ thuật đối chiếu, chưa bật thu hoặc ACK.".into());
            }
            if config.journal_path.is_some()
                && (!journal.starts_with(&runtime)
                    || journal
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir)))
            {
                return Err("Journal phải nằm trong local-runtime cạnh ứng dụng. Giữ nguyên lịch sử cũ và yêu cầu kỹ thuật đối chiếu; không tự di chuyển.".into());
            }
        }
    }
    if runtime.exists() {
        inspect(&runtime)?;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(database::DatabaseState::default())
        .manage(plc::worker::WorkerState::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            let app_handle = app.handle().clone();
            let database_state = app.state::<database::DatabaseState>().inner().clone();
            let worker = app.state::<plc::worker::WorkerState>().inner().clone();
            let worker_database = database_state.clone();
            let config_dir = application_directory().and_then(|dir| {
                check_plc_directory(&dir)?;
                plc::connection_settings::ensure_config(&dir)?;
                Ok(dir)
            });
            tauri::async_runtime::spawn(async move {
                // Acquisition needs the migrated pool; do not race database startup.
                database_state.initialize(&app_handle).await;
                match config_dir {
                    Ok(config_dir) => worker.initialize(Some(config_dir), worker_database).await,
                    Err(error) => worker.report_runtime_initialization_failure(error).await,
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::measurement_command::get_database_status,
            commands::measurement_command::retry_database_connection,
            commands::measurement_command::get_measurements,
            commands::measurement_command::get_measurement_by_id,
            commands::measurement_command::get_measurement_statistics,
            commands::measurement_command::get_measurement_quality_statistics,
            commands::measurement_command::get_measurement_skus,
            commands::measurement_command::get_measurement_sku_summary,
            commands::measurement_command::get_measurement_history_page,
            commands::measurement_command::get_measurement_report,
            plc::worker::get_as200_worker_status,
            plc::worker::reconnect_as200_plc,
            plc::connection_settings::get_plc_connection_settings,
            plc::connection_settings::stop_plc_collection,
            plc::connection_settings::clear_plc_connection_log,
            plc::connection_settings::connect_plc_collection,
            plc::connection_settings::test_plc_connection,
            plc::worker::retry_as200_inbox,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod storage_tests {
    use super::*;

    #[test]
    fn application_storage_does_not_import_or_replace_missing_history() {
        let dir = std::env::temp_dir().join(format!(
            "golf-storage-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        std::fs::create_dir(&dir).unwrap();
        assert!(check_plc_directory(&dir).is_ok());
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
        assert!(plc::simulator_config::load(Some(&dir)).unwrap().is_none());
        let runtime = dir.join("local-runtime");
        std::fs::create_dir(&runtime).unwrap();
        let journal = runtime.join("capture.json");
        std::fs::write(&journal, b"preserve-history").unwrap();
        assert!(check_plc_directory(&dir).is_err());
        assert_eq!(std::fs::read(&journal).unwrap(), b"preserve-history");
        assert!(!dir.join("as200_config.toml").exists());
        std::fs::remove_file(journal).unwrap();
        std::fs::remove_dir(runtime).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
