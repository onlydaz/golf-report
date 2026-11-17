use sqlx::{FromRow, PgPool};

pub const INITIAL_VERSION: i64 = 20261109120000;
pub const REGISTRY_VERSION: i64 = 20261111120000;
pub const PUBLICATION_VERSION: i64 = 20261117120000;
pub const REGISTRY_RETIREMENT_VERSION: i64 = 20261117140000;
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
    eprintln!(
        "DATABASE_MIGRATION state=CHECKING embedded={:?} applied={:?} tables={tables:?}",
        MIGRATOR.iter().map(|m| m.version).collect::<Vec<_>>(),
        history.iter().map(|m| m.version).collect::<Vec<_>>(),
    );
    if let Err(error) = validate_history(&tables, &history) {
        let location: (String, String, String) = sqlx::query_as(
            "SELECT current_database()::text, current_schema()::text, current_setting('search_path')",
        )
        .fetch_one(pool)
        .await
        .map_err(|e| format!("{error} Không thể đọc database/schema hiện tại: {e}"))?;
        let found: Vec<(String, String)> = sqlx::query_as(
            "SELECT n.nspname::text, c.relname::text FROM pg_class c \
             JOIN pg_namespace n ON n.oid=c.relnamespace \
             WHERE c.relkind IN ('r','p') AND c.relname IN \
             ('_sqlx_migrations','plc_sources','plc_inbox','measurements', \
              'measurement_stations','measurement_points','measurement_attempts','plc_source_registry') \
             ORDER BY n.nspname,c.relname",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| format!("{error} Không thể tra cứu vị trí bảng: {e}"))?;
        return Err(format!(
            "{error} Database={}, schema={}, search_path={}, bảng tìm thấy={found:?}.",
            location.0, location.1, location.2
        ));
    }
    eprintln!("DATABASE_MIGRATION state=APPLYING_PENDING");
    MIGRATOR
        .run(pool)
        .await
        .map_err(|e| format!("Migration AS200 thất bại (giữ nguyên lịch sử): {e}"))?;
    let (tables, history) = audit(pool).await?;
    validate_history(&tables, &history)?;
    eprintln!("DATABASE_MIGRATION state=COMPLETE");
    Ok(())
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
    // A retirement row alone is not proof: all preceding embedded versions must exist.
    if let Some(last) = history.iter().map(|item| item.version).max() {
        for migration in MIGRATOR.iter().filter(|m| m.version <= last) {
            if history
                .iter()
                .filter(|h| h.version == migration.version)
                .count()
                != 1
            {
                return Err(format!(
                    "Lịch sử SQLx thiếu hoặc trùng version {}; không tự sửa lịch sử.",
                    migration.version
                ));
            }
        }
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
        let registry_retired = history
            .iter()
            .any(|item| item.version == REGISTRY_RETIREMENT_VERSION);
        if history.iter().any(|item| item.version == REGISTRY_VERSION)
            && !registry_retired
            && !tables.iter().any(|name| name == "plc_source_registry")
        {
            return Err("Lịch sử SQLx ghi registry đã áp dụng nhưng thiếu bảng plc_source_registry. Không tự tạo lại bảng khi chưa đối chiếu schema và dữ liệu.".into());
        }
        if registry_retired && tables.iter().any(|name| name == "plc_source_registry") {
            return Err(
                "SQLx ghi registry đã retire nhưng bảng vẫn tồn tại; cần đối chiếu schema/history."
                    .into(),
            );
        }
        if history
            .iter()
            .any(|item| item.version == PUBLICATION_VERSION)
            && !tables.iter().any(|name| name == "plc_acquisition")
        {
            return Err("Lịch sử SQLx ghi publication đã áp dụng nhưng thiếu plc_acquisition; không tự tạo lại checkpoint.".into());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "migration_tests.rs"]
mod integration_tests;

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
