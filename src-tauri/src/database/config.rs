use serde::Deserialize;
use sqlx::postgres::{PgConnectOptions, PgSslMode};
use std::fs;
use std::str::FromStr;

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

impl DatabaseConfig {
    pub fn connect_options(&self) -> Result<PgConnectOptions, String> {
        let has_fields = !self.host.is_empty()
            || self.port != 0
            || !self.database.is_empty()
            || !self.username.is_empty()
            || !self.password.is_empty();
        if let Some(url) = &self.connection_string {
            if has_fields {
                return Err("Cấu hình PostgreSQL có cả connection_string và các trường kết nối riêng. Để dùng Local, hãy bỏ connection_string và giữ host, port, database, username, password.".into());
            }
            if url.trim().is_empty() {
                return Err("Connection string PostgreSQL đang trống.".into());
            }
            return PgConnectOptions::from_str(url)
                .map(|options| options.ssl_mode(PgSslMode::VerifyFull))
                .map_err(|_| "Connection string PostgreSQL không hợp lệ.".into());
        }
        for (name, value) in [
            ("host", &self.host),
            ("database", &self.database),
            ("username", &self.username),
        ] {
            if value.trim().is_empty() || value.chars().any(char::is_control) {
                return Err(format!(
                    "Thiếu hoặc không hợp lệ trường {name} trong cấu hình PostgreSQL."
                ));
            }
        }
        if self.port == 0 {
            return Err("Thiếu hoặc không hợp lệ trường port PostgreSQL (1–65535).".into());
        }
        if !self
            .host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'))
        {
            return Err(
                "Trường host PostgreSQL phải là hostname hoặc IP, không phải connection string."
                    .into(),
            );
        }
        if self.password.is_empty() {
            return Err("Thiếu hoặc rỗng trường password trong cấu hình PostgreSQL.".into());
        }
        Ok(PgConnectOptions::new()
            .host(&self.host)
            .port(self.port)
            .database(&self.database)
            .username(&self.username)
            .password(&self.password)
            .ssl_mode(PgSslMode::Disable))
    }
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
    config.connect_options()?;
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
            "host = 'localhost'\nport = 5432\ndatabase = 'golf_report'\nusername = 'postgres'\npassword = 'fixture-only'",
        )
        .unwrap();
        assert!(config.connection_string.is_none());
        assert_eq!(config.port, 5432);
        assert_eq!(config.password, "fixture-only");
        let options = config.connect_options().unwrap();
        assert_eq!(options.get_host(), "localhost");
        assert_eq!(options.get_database(), Some("golf_report"));
        assert_eq!(options.get_username(), "postgres");
        assert!(matches!(options.get_ssl_mode(), PgSslMode::Disable));
    }

    #[test]
    fn validates_required_local_fields_without_disclosing_values() {
        let valid = "host='localhost'\nport=5432\ndatabase='golf_report'\nusername='postgres'\npassword='fixture-secret'";
        for line in valid.lines() {
            let missing = valid
                .lines()
                .filter(|candidate| *candidate != line)
                .collect::<Vec<_>>()
                .join("\n");
            let config: DatabaseConfig = toml::from_str(&missing).unwrap();
            let error = config.connect_options().err().unwrap();
            assert!(error.contains(line.split('=').next().unwrap()));
            assert!(!error.contains("fixture-secret"));
        }
        for invalid in ["port=-1", "port=65536", "port='5432'"] {
            assert!(toml::from_str::<DatabaseConfig>(invalid).is_err());
        }
        let mixed: DatabaseConfig = toml::from_str(&format!(
            "{valid}\nconnection_string='postgresql://old:secret@example.invalid/old'"
        ))
        .unwrap();
        let error = mixed.connect_options().err().unwrap();
        assert!(error.contains("bỏ connection_string"));
        assert!(!error.contains("old:secret"));
        let cloud: DatabaseConfig =
            toml::from_str("connection_string='postgresql://fixture:secret@example.invalid/cloud'")
                .unwrap();
        assert!(matches!(
            cloud.connect_options().unwrap().get_ssl_mode(),
            PgSslMode::VerifyFull
        ));
    }
}
