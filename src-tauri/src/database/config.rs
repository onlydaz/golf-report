use serde::Deserialize;
use std::{fs, path::PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: String,
}

pub fn load_config(app_config_dir: Option<PathBuf>) -> Result<DatabaseConfig, String> {
    let mut candidates = Vec::new();
    if let Some(path) = app_config_dir {
        candidates.push(path.join("database_config.toml"));
    }
    if let Ok(current_dir) = std::env::current_dir() {
        candidates.push(current_dir.join("database_config.toml"));
        candidates.push(current_dir.join("src-tauri").join("database_config.toml"));
    }
    let config_path = candidates.into_iter().find(|path| path.is_file()).ok_or_else(|| {
        "Chưa tìm thấy database_config.toml. Hãy tạo file cấu hình trong thư mục cấu hình ứng dụng.".to_string()
    })?;
    let content = fs::read_to_string(&config_path).map_err(|_| "Không thể đọc file cấu hình cơ sở dữ liệu.".to_string())?;
    toml::from_str::<DatabaseConfig>(&content).map_err(|_| "File cấu hình cơ sở dữ liệu không hợp lệ.".to_string())
}
