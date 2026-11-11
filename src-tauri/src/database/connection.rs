use crate::database::config::load_config;
use serde::Serialize;
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DatabaseConnectionState {
    Connecting,
    Connected,
    ConnectionFailed,
    ConnectionLost,
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
    health_lock: Arc<Mutex<Option<Instant>>>,
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
            health_lock: Arc::new(Mutex::new(None)),
        }
    }
}

impl DatabaseState {
    pub async fn status(&self) -> DatabaseStatus {
        let mut checked = self.health_lock.lock().await;
        let pool = self.inner.lock().await.pool.clone();
        if let Some(pool) = pool {
            if checked.is_none_or(|last| last.elapsed() >= Duration::from_secs(1)) {
                let healthy = health_with_timeout(
                    async {
                        sqlx::query_scalar::<_, i32>("SELECT 1")
                            .fetch_one(&pool)
                            .await
                            .is_ok()
                    },
                    Duration::from_secs(2),
                )
                .await;
                self.record_health(healthy).await;
                *checked = Some(Instant::now());
            }
        }
        self.inner.lock().await.status.clone()
    }

    async fn record_health(&self, healthy: bool) {
        self.set_status(
            if healthy { DatabaseConnectionState::Connected } else { DatabaseConnectionState::ConnectionLost },
            if healthy { "Cơ sở dữ liệu đã kết nối." } else { "Đã mất kết nối cơ sở dữ liệu. Đang chờ PostgreSQL phục hồi; có thể thử kết nối lại." },
        ).await;
    }

    pub async fn initialize(&self, app: &AppHandle) -> DatabaseStatus {
        let _initialization_guard = self.initialization_lock.lock().await;
        // Reuse an existing migrated pool; SQLx reconnects its sockets on recovery.
        if self.inner.lock().await.pool.is_some() {
            *self.health_lock.lock().await = None;
            return self.status().await;
        }

        self.set_status(
            DatabaseConnectionState::Connecting,
            "Đang kết nối cơ sở dữ liệu...",
        )
        .await;
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

    async fn connect_and_migrate(
        &self,
        app: &AppHandle,
    ) -> Result<PgPool, (DatabaseConnectionState, String)> {
        let app_config_dir = app.path().app_config_dir().ok();
        let config = load_config(app_config_dir)
            .map_err(|message| (DatabaseConnectionState::ConnectionFailed, message))?;
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
            .acquire_timeout(Duration::from_secs(3))
            .connect(&connection_string)
            .await
            .map_err(|_| {
                (
                    DatabaseConnectionState::ConnectionFailed,
                    "Không thể kết nối cơ sở dữ liệu. Hãy kiểm tra PostgreSQL và cấu hình."
                        .to_string(),
                )
            })?;
        super::migrations::migrate(&pool)
            .await
            .map_err(|message| (DatabaseConnectionState::MigrationFailed, message))?;
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
        match &inner.pool {
            Some(pool) => Ok(pool.clone()),
            None => Err(inner.status.message.clone()),
        }
    }
}

async fn health_with_timeout(
    probe: impl std::future::Future<Output = bool>,
    timeout: Duration,
) -> bool {
    tokio::time::timeout(timeout, probe).await.unwrap_or(false)
}

#[cfg(test)]
mod health_tests {
    use super::*;
    #[tokio::test]
    #[ignore = "Opt-in AS200_READONLY_DATABASE=golf_report; SELECT only, no database outage or DML"]
    async fn postgres_health_probe_recovers_status_on_the_same_pool_read_only() {
        let target =
            std::env::var("AS200_READONLY_DATABASE").expect("Specify read-only audit database");
        assert_eq!(target, "golf_report");
        let app_dir = std::env::var_os("APPDATA")
            .map(|p| std::path::PathBuf::from(p).join("com.golf.report"));
        let config = load_config(app_dir).unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(
                sqlx::postgres::PgConnectOptions::new()
                    .host(&config.host)
                    .port(config.port)
                    .database(&target)
                    .username(&config.username)
                    .password(&config.password)
                    .options([("default_transaction_read_only", "on")]),
            )
            .await
            .unwrap();
        let read_only: String = sqlx::query_scalar("SHOW transaction_read_only")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(read_only, "on");
        let state = DatabaseState::default();
        state.inner.lock().await.pool = Some(pool.clone());
        assert!(matches!(
            state.status().await.state,
            DatabaseConnectionState::Connected
        ));
        state.record_health(false).await; // Simulate a failed probe, never stop the real DB.
        *state.health_lock.lock().await = None;
        assert!(matches!(
            state.status().await.state,
            DatabaseConnectionState::Connected
        ));
        assert_eq!(state.pool().await.unwrap().size(), 1);
        pool.close().await;
        assert!(state.pool().await.unwrap().is_closed());
    }
    #[tokio::test]
    async fn timeout_loss_and_recovery_reuse_the_existing_pool() {
        assert!(!health_with_timeout(std::future::pending(), Duration::from_millis(5)).await);
        assert!(health_with_timeout(async { true }, Duration::from_millis(5)).await);
        let state = DatabaseState::default();
        assert!(matches!(
            state.status().await.state,
            DatabaseConnectionState::Connecting
        ));
        state
            .set_status(
                DatabaseConnectionState::ConnectionFailed,
                "Initial connection failed",
            )
            .await;
        assert!(matches!(
            state.status().await.state,
            DatabaseConnectionState::ConnectionFailed
        ));
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap();
        pool.close().await; // No connection is attempted by this test.
        state.inner.lock().await.pool = Some(pool.clone());
        assert!(matches!(
            state.status().await.state,
            DatabaseConnectionState::ConnectionLost
        ));
        assert!(state.pool().await.unwrap().is_closed());
        state.record_health(true).await;
        assert!(matches!(
            state.inner.lock().await.status.state,
            DatabaseConnectionState::Connected
        ));
        assert!(state.pool().await.unwrap().is_closed()); // Same pool, not a replacement.
    }
}
