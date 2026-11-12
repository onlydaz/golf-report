use serde::Deserialize;
use std::{fs, path::PathBuf};

// Compiled into the installer executable; the source TOML stays outside Git.
const BUNDLED_CONFIG: &str = include_str!("../../resources/database_config.toml");

#[derive(Clone, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default)]
    pub connection_string: Option<String>,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}

pub fn load_config(app_config_dir: Option<PathBuf>) -> Result<DatabaseConfig, String> {
    let config_path = app_config_dir.map(|dir| dir.join("database_config.toml"));
    let content = if !cfg!(debug_assertions) {
        BUNDLED_CONFIG.to_string()
    } else if let Some(path) = config_path.as_ref().filter(|path| path.is_file()) {
        fs::read_to_string(path)
            .map_err(|_| "Không thể đọc file cấu hình PostgreSQL trong AppData.".to_string())?
    } else {
        BUNDLED_CONFIG.to_string()
    };
    let config = toml::from_str::<DatabaseConfig>(&content)
        .map_err(|_| "File cấu hình cơ sở dữ liệu không hợp lệ.".to_string())?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::DatabaseConfig;

    #[test]
    fn accepts_cloud_connection_string_without_legacy_fields() {
        let config: DatabaseConfig = toml::from_str(
            "connection_string = 'postgresql://user:password@example.invalid/golf_report?sslmode=require'",
        )
        .unwrap();
        assert!(config.connection_string.is_some());
        assert!(config.host.is_empty());
    }

    #[test]
    fn accepts_existing_local_config() {
        let config: DatabaseConfig = toml::from_str(
            "host = 'localhost'\nport = 5432\ndatabase = 'golf_report'\nusername = 'postgres'\npassword = ''",
        )
        .unwrap();
        assert!(config.connection_string.is_none());
        assert_eq!(config.port, 5432);
    }
}
