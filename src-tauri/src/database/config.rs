use serde::Deserialize;
use std::fs;

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

pub fn load_config() -> Result<DatabaseConfig, String> {
    #[cfg(debug_assertions)]
    let config_path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/database_config.toml");
    #[cfg(not(debug_assertions))]
    let config_path = crate::application_directory()?.join("resources/database_config.toml");
    // Log only the selected path, never config contents or the connection URL.
    eprintln!("DATABASE_CONFIG path={}", config_path.display());
    read_config(&config_path)
}

fn read_config(config_path: &std::path::Path) -> Result<DatabaseConfig, String> {
    let content = fs::read_to_string(config_path).map_err(|_| {
        "Thiếu hoặc không đọc được resources/database_config.toml trong thư mục ứng dụng (Dev: src-tauri/resources). Hãy kiểm tra file và thử kết nối lại.".to_string()
    })?;
    let config = toml::from_str::<DatabaseConfig>(&content)
        .map_err(|_| "File cấu hình cơ sở dữ liệu không hợp lệ.".to_string())?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_config_is_preserved_and_invalid_config_never_falls_back_or_leaks() {
        let dir = std::env::temp_dir().join(format!(
            "golf-report-config-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("database_config.toml");
        let content = "connection_string = 'postgresql://fixture:dummy@example.invalid/external'";
        fs::write(&path, content).unwrap();
        let config = read_config(&path).unwrap();
        assert_eq!(
            config.connection_string.as_deref(),
            Some("postgresql://fixture:dummy@example.invalid/external")
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), content);

        let invalid = "connection_string = 'fixture-secret-not-to-log";
        fs::write(&path, invalid).unwrap();
        let error = read_config(&path).err().unwrap();
        assert_eq!(error, "File cấu hình cơ sở dữ liệu không hợp lệ.");
        assert!(!error.contains("fixture-secret"));
        assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }

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
