use crate::database::config::load_config;
use serde::Serialize;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DatabaseConnectionState {
    Connecting,
    Connected,
    ConnectionFailed,
    MigrationFailed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseStatus {
    pub state: DatabaseConnectionState,
    pub message: String,
}

struct DatabaseInner {
    pool: Option<PgPool>,
    status: DatabaseStatus,
}

#[derive(Clone)]
pub struct DatabaseState {
    inner: Arc<Mutex<DatabaseInner>>,
    initialization_lock: Arc<Mutex<()>>,
}

impl Default for DatabaseState {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(DatabaseInner {
                pool: None,
                status: DatabaseStatus {
                    state: DatabaseConnectionState::Connecting,
                    message: "Đang kết nối cơ sở dữ liệu...".to_string(),
                },
            })),
            initialization_lock: Arc::new(Mutex::new(())),
        }
    }
}

impl DatabaseState {
    #[cfg(test)]
    pub(crate) async fn with_test_pool(pool: PgPool) -> Self {
        let state = Self::default();
        {
            let mut inner = state.inner.lock().await;
            inner.pool = Some(pool);
            inner.status.state = DatabaseConnectionState::Connected;
        }
        state
    }
    pub async fn status(&self) -> DatabaseStatus {
        self.inner.lock().await.status.clone()
    }

    pub async fn initialize(&self, app: &AppHandle) -> DatabaseStatus {
        let _initialization_guard = self.initialization_lock.lock().await;
        if matches!(self.status().await.state, DatabaseConnectionState::Connected) {
            return self.status().await;
        }

        self.set_status(DatabaseConnectionState::Connecting, "Đang kết nối cơ sở dữ liệu...").await;
        let result = self.connect_and_migrate(app).await;
        match result {
            Ok(pool) => {
                let mut inner = self.inner.lock().await;
                inner.pool = Some(pool);
                inner.status = DatabaseStatus {
                    state: DatabaseConnectionState::Connected,
                    message: "Cơ sở dữ liệu đã kết nối.".to_string(),
                };
            }
            Err((state, message)) => {
                let mut inner = self.inner.lock().await;
                inner.pool = None;
                inner.status = DatabaseStatus { state, message };
            }
        }
        self.status().await
    }

    async fn connect_and_migrate(&self, app: &AppHandle) -> Result<PgPool, (DatabaseConnectionState, String)> {
        let app_config_dir = app.path().app_config_dir().ok();
        let config = load_config(app_config_dir).map_err(|message| {
            (DatabaseConnectionState::ConnectionFailed, message)
        })?;
        let connection_string = format!(
            "postgres://{}:{}@{}:{}/{}",
            urlencoding::encode(&config.username),
            urlencoding::encode(&config.password),
            config.host,
            config.port,
            config.database
        );
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&connection_string)
            .await
            .map_err(|_| (
                DatabaseConnectionState::ConnectionFailed,
                "Không thể kết nối cơ sở dữ liệu. Hãy kiểm tra PostgreSQL và cấu hình.".to_string(),
            ))?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|_| (
                DatabaseConnectionState::MigrationFailed,
                "Kết nối thành công nhưng không thể hoàn tất migration cơ sở dữ liệu.".to_string(),
            ))?;
        Ok(pool)
    }

    async fn set_status(&self, state: DatabaseConnectionState, message: &str) {
        self.inner.lock().await.status = DatabaseStatus {
            state,
            message: message.to_string(),
        };
    }

    pub async fn pool(&self) -> Result<PgPool, String> {
        let inner = self.inner.lock().await;
        match (&inner.pool, &inner.status.state) {
            (Some(pool), DatabaseConnectionState::Connected) => Ok(pool.clone()),
            (_, _) => Err(inner.status.message.clone()),
        }
    }
}
