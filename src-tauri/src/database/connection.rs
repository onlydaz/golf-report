use crate::database::config::load_config;
use serde::Serialize;
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::AppHandle;
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

fn server_error_message(code: &str, endpoint: &str) -> Option<String> {
    match code {
        "28P01" | "28000" => Some(format!(
            "PostgreSQL tại {endpoint} từ chối xác thực. Kiểm tra tài khoản, mật khẩu và quyền kết nối."
        )),
        "3D000" => Some(format!(
            "Database được cấu hình không tồn tại trên PostgreSQL tại {endpoint}. Kiểm tra trường database; ứng dụng không tự tạo database."
        )),
        "53300" => Some(format!(
            "PostgreSQL tại {endpoint} đã hết kết nối khả dụng. Hãy thử lại sau."
        )),
        "57P03" => Some(format!(
            "PostgreSQL tại {endpoint} chưa sẵn sàng nhận kết nối. Kiểm tra dịch vụ và thử lại."
        )),
        _ => None,
    }
}

/// Never forward driver errors: they may contain usernames or server details.
pub fn connection_error(error: &sqlx::Error, endpoint: &str) -> String {
    if let Some(message) = error
        .as_database_error()
        .and_then(|error| error.code())
        .and_then(|code| server_error_message(&code, endpoint))
    {
        return message;
    }
    match error {
        sqlx::Error::Io(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => format!(
            "PostgreSQL tại {endpoint} từ chối kết nối TCP. Kiểm tra dịch vụ PostgreSQL đang chạy và đúng cổng cấu hình."
        ),
        sqlx::Error::PoolTimedOut => format!(
            "Hết thời gian kết nối PostgreSQL tại {endpoint}. Kiểm tra dịch vụ PostgreSQL, cổng, firewall, tài khoản, mật khẩu và database."
        ),
        sqlx::Error::Tls(_) => format!(
            "Không thể xác minh kết nối TLS PostgreSQL tại {endpoint}. Kiểm tra cấu hình TLS và chứng chỉ máy chủ."
        ),
        _ => format!(
            "Không thể kết nối PostgreSQL tại {endpoint}. Kiểm tra dịch vụ PostgreSQL, tài khoản, mật khẩu và database."
        ),
    }
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
                eprintln!("DATABASE_STARTUP state={state:?} reason={message}");
                let mut inner = self.inner.lock().await;
                inner.pool = None;
                inner.status = DatabaseStatus { state, message };
            }
        }
        self.status().await
    }

    async fn connect_and_migrate(
        &self,
        _app: &AppHandle,
    ) -> Result<PgPool, (DatabaseConnectionState, String)> {
        let config = load_config()
            .map_err(|message| (DatabaseConnectionState::ConnectionFailed, message))?;
        let cloud = config.connection_string.is_some();
        let options = config
            .connect_options()
            .map_err(|message| (DatabaseConnectionState::ConnectionFailed, message))?;
        let endpoint = format!("{}:{}", options.get_host(), options.get_port());
        eprintln!(
            "DATABASE_CONNECT host={} port={} database={} ssl={:?}",
            options.get_host(),
            options.get_port(),
            options.get_database().unwrap_or("<default>"),
            options.get_ssl_mode(),
        );
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(if cloud { 15 } else { 3 }))
            .connect_with(options)
            .await
            .map_err(|error| {
                (
                    DatabaseConnectionState::ConnectionFailed,
                    connection_error(&error, &endpoint),
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
    #[test]
    fn connection_errors_are_specific_and_do_not_expose_driver_details() {
        let endpoint = "localhost:5432";
        assert!(
            server_error_message("28P01", endpoint)
                .unwrap()
                .contains("xác thực")
        );
        assert!(
            server_error_message("28000", endpoint)
                .unwrap()
                .contains("xác thực")
        );
        assert!(
            server_error_message("3D000", endpoint)
                .unwrap()
                .contains("không tồn tại")
        );
        assert!(
            server_error_message("57P03", endpoint)
                .unwrap()
                .contains("chưa sẵn sàng")
        );
        assert!(server_error_message("unknown", endpoint).is_none());
        for error in [
            sqlx::Error::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "fixture-password",
            )),
            sqlx::Error::PoolTimedOut,
        ] {
            let message = connection_error(&error, endpoint);
            assert!(message.contains(endpoint));
            assert!(!message.contains("fixture-password"));
            assert!(!message.contains("Cloud"));
        }
    }
    #[tokio::test]
    #[ignore = "Explicit Neon integration opt-in; SELECT only, no real database outage"]
    async fn postgres_health_probe_recovers_status_on_the_same_pool_read_only() {
        let pool = crate::database::test_support::readonly_pool(1).await;
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
