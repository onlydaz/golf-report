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

#[tauri::command]
pub async fn get_measurement_quality_statistics(
    state: State<'_, DatabaseState>,
    request: Option<MeasurementListRequest>,
) -> Result<crate::models::MeasurementQualityStatistics, String> {
    let pool = state.pool().await?;
    crate::repositories::MeasurementRepository::quality_statistics(
        &pool,
        &request.unwrap_or_default(),
    )
    .await
}

#[tauri::command]
pub async fn get_measurement_skus(
    state: State<'_, DatabaseState>,
    search: Option<String>,
    page: Option<i64>,
) -> Result<crate::models::MeasurementSkuOptions, String> {
    let pool = state.pool().await?;
    crate::repositories::MeasurementRepository::sku_options(
        &pool,
        search.as_deref().unwrap_or(""),
        page.unwrap_or(1),
    )
    .await
}
#[tauri::command]
pub async fn get_measurement_sku_summary(
    state: State<'_, DatabaseState>,
    request: Option<MeasurementListRequest>,
) -> Result<crate::models::MeasurementSkuPage, String> {
    let pool = state.pool().await?;
    crate::repositories::MeasurementRepository::sku_summary(&pool, &request.unwrap_or_default())
        .await
}
#[tauri::command]
pub async fn get_measurement_history_page(
    state: State<'_, DatabaseState>,
    request: Option<MeasurementListRequest>,
) -> Result<crate::models::MeasurementHistoryPage, String> {
    let pool = state.pool().await?;
    crate::repositories::MeasurementRepository::history_page(&pool, &request.unwrap_or_default())
        .await
}
