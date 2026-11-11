mod commands;
pub mod database;
pub mod enums;
pub mod models;
mod plc;
pub mod repositories;
pub mod services;

use tauri::Manager;

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
            let config_dir = app.path().app_config_dir().ok();
            tauri::async_runtime::spawn(async move {
                worker.launch(config_dir, worker_database).await;
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
            plc::worker::retry_as200_inbox,
            plc::worker::get_as200_retry_diagnostics,
            plc::worker::acknowledge_as200_buffer_gap
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
