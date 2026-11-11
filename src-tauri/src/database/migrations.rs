use sqlx::{FromRow, PgPool};

pub const INITIAL_VERSION: i64 = 20261109120000;
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, FromRow)]
pub struct MigrationHistory {
    pub version: i64,
    pub description: String,
    pub success: bool,
}
pub async fn audit(pool: &PgPool) -> Result<(Vec<String>, Vec<MigrationHistory>), String> {
    let tables: Vec<(String,)> = sqlx::query_as("SELECT tablename::text FROM pg_tables WHERE schemaname=current_schema() ORDER BY tablename")
        .fetch_all(pool).await.map_err(|e| e.to_string())?;
    let tables: Vec<String> = tables.into_iter().map(|row| row.0).collect();
    let history = if tables.iter().any(|name| name == "_sqlx_migrations") {
        sqlx::query_as("SELECT version,description,success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(pool)
            .await
            .map_err(|e| e.to_string())?
    } else {
        Vec::new()
    };
    Ok((tables, history))
}
pub async fn migrate(pool: &PgPool) -> Result<(), String> {
    let (tables, history) = audit(pool).await?;
    validate_history(&tables, &history)?;
    MIGRATOR
        .run(pool)
        .await
        .map_err(|e| format!("Migration AS200 thất bại (giữ nguyên lịch sử): {e}"))
}
pub fn validate_history(tables: &[String], history: &[MigrationHistory]) -> Result<(), String> {
    if history
        .iter()
        .any(|item| !MIGRATOR.iter().any(|known| known.version == item.version) || !item.success)
    {
        return Err(format!(
            "Lịch sử _sqlx_migrations không tương thích AS200: {history:?}. Không xóa lịch sử tự động; cần database development trống hoặc xử lý migration riêng."
        ));
    }
    if history.is_empty() && tables.iter().any(|name| name != "_sqlx_migrations") {
        return Err(format!(
            "Database chưa có lịch sử AS200 nhưng còn bảng: {tables:?}. Không tự xóa hoặc chạy chồng schema."
        ));
    }
    if !history.is_empty() {
        if !history.iter().any(|item| item.version == INITIAL_VERSION) {
            return Err(
                "Lịch sử thiếu migration khởi tạo AS200; không tự sửa hoặc bỏ qua lịch sử.".into(),
            );
        }
        for table in [
            "plc_sources",
            "plc_inbox",
            "measurements",
            "measurement_stations",
            "measurement_points",
            "measurement_attempts",
        ] {
            if !tables.iter().any(|name| name == table) {
                return Err(format!(
                    "Lịch sử SQLx ghi AS200 đã áp dụng nhưng thiếu bảng {table}. Không tự reset lịch sử hoặc chạy chồng migration."
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_preflight_never_silently_replaces_old_history_or_tables() {
        assert!(validate_history(&[], &[]).is_ok());
        assert!(validate_history(&["_sqlx_migrations".into()], &[]).is_ok());
        assert!(validate_history(&["tip_positions".into()], &[]).is_err());
        assert!(
            validate_history(
                &[],
                &[MigrationHistory {
                    version: 20261104120000,
                    description: "old".into(),
                    success: true
                }]
            )
            .is_err()
        );
        assert!(
            validate_history(
                &[],
                &[MigrationHistory {
                    version: INITIAL_VERSION,
                    description: "as200".into(),
                    success: true
                }]
            )
            .is_err()
        );
    }
}
