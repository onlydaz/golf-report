mod commands;
mod database;
mod enums;
mod models;
mod repositories;
mod services;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(database::DatabaseState::default())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            let app_handle = app.handle().clone();
            let database_state = app.state::<database::DatabaseState>().inner().clone();
            tauri::async_runtime::spawn(async move {
                database_state.initialize(&app_handle).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::measurement_command::check_database_connection,
            commands::measurement_command::get_database_status,
            commands::measurement_command::retry_database_connection,
            commands::measurement_command::create_measurement,
            commands::measurement_command::get_measurements,
            commands::measurement_command::get_measurement_by_id,
            commands::measurement_command::get_measurement_statistics
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
