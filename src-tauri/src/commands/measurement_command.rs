use crate::database::{DatabaseState, DatabaseStatus};
use crate::models::{
    Measurement, MeasurementListRequest, MeasurementListResponse, MeasurementStatistics,
};
use crate::services::MeasurementService;
use tauri::{AppHandle, State};

#[tauri::command]
pub async fn check_database_connection(
    app: AppHandle,
    state: State<'_, DatabaseState>,
) -> Result<String, String> {
    let status = state.initialize(&app).await;
    match status.state {
        crate::database::connection::DatabaseConnectionState::Connected => Ok(status.message),
        _ => Err(status.message),
    }
}

#[tauri::command]
pub async fn get_database_status(
    state: State<'_, DatabaseState>,
) -> Result<DatabaseStatus, String> {
    Ok(state.status().await)
}

#[tauri::command]
pub async fn retry_database_connection(
    app: AppHandle,
    state: State<'_, DatabaseState>,
) -> Result<DatabaseStatus, String> {
    Ok(state.initialize(&app).await)
}

#[tauri::command]
pub async fn get_measurements(
    state: State<'_, DatabaseState>,
    request: Option<MeasurementListRequest>,
) -> Result<MeasurementListResponse, String> {
    let pool = state.pool().await?;
    MeasurementService::list(&pool, request.unwrap_or_default()).await
}

#[tauri::command]
pub async fn get_measurement_by_id(
    state: State<'_, DatabaseState>,
    id: i64,
) -> Result<Measurement, String> {
    let pool = state.pool().await?;
    MeasurementService::find_by_id(&pool, id).await
}

#[tauri::command]
pub async fn get_measurement_statistics(
    state: State<'_, DatabaseState>,
    request: Option<MeasurementListRequest>,
) -> Result<MeasurementStatistics, String> {
    let pool = state.pool().await?;
    MeasurementService::statistics(&pool, request.unwrap_or_default()).await
}

#[tauri::command]
pub async fn get_measurement_report(
    state: State<'_, DatabaseState>,
    request: Option<MeasurementListRequest>,
) -> Result<Vec<Measurement>, String> {
    let pool = state.pool().await?;
    crate::repositories::MeasurementRepository::report(&pool, &request.unwrap_or_default()).await
}
