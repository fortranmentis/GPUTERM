use crate::ssh::session::config_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_local_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalEntry {
    name: String,
    path: String,
    entry_type: String,
    size: Option<u64>,
    modified_time: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalListResponse {
    path: String,
    entries: Vec<LocalEntry>,
}

#[tauri::command]
pub fn load_app_settings() -> Result<AppSettings, String> {
    read_app_settings()
}

#[tauri::command]
pub fn update_recent_local_path(path: String) -> Result<AppSettings, String> {
    let normalized = validate_local_directory(&path)?;
    let mut settings = read_app_settings()?;
    settings.recent_local_path = Some(normalized);
    write_app_settings(&settings)?;
    Ok(settings)
}

#[tauri::command]
pub fn list_local_dir(path: String) -> Result<LocalListResponse, String> {
    let normalized = validate_local_directory(&path)?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(&normalized)
        .map_err(|error| format!("Failed to read local path {}: {}", normalized, error))?
    {
        let entry = entry.map_err(|error| format!("Failed to read local entry: {}", error))?;
        let path = entry.path();
        let metadata = entry
            .metadata()
            .map_err(|error| format!("Failed to inspect local entry {}: {}", path.display(), error))?;
        let name = entry.file_name().to_string_lossy().to_string();
        let entry_type = if metadata.is_dir() {
            "directory"
        } else if metadata.is_file() {
            "file"
        } else {
            "other"
        };
        let modified_time = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs());
        entries.push(LocalEntry {
            name,
            path: path.to_string_lossy().to_string(),
            entry_type: entry_type.to_string(),
            size: metadata.is_file().then_some(metadata.len()),
            modified_time,
        });
    }

    entries.sort_by(|a, b| {
        let left_dir = a.entry_type == "directory";
        let right_dir = b.entry_type == "directory";
        right_dir
            .cmp(&left_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(LocalListResponse {
        path: normalized,
        entries,
    })
}

#[tauri::command]
pub fn local_path_exists(path: String) -> Result<bool, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("Local path is required".to_string());
    }
    Ok(PathBuf::from(trimmed).exists())
}

fn validate_local_directory(path: &str) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("Local path is required".to_string());
    }

    let path = PathBuf::from(trimmed);
    let canonical = fs::canonicalize(&path)
        .map_err(|error| format!("Local path is unavailable or does not exist: {}", error))?;
    let metadata = fs::metadata(&canonical)
        .map_err(|error| format!("Local path is not accessible: {}", error))?;
    if !metadata.is_dir() {
        return Err("Local path must be a directory".to_string());
    }
    Ok(canonical.to_string_lossy().to_string())
}

fn read_app_settings() -> Result<AppSettings, String> {
    let path = settings_path();
    if !path.exists() {
        return Ok(AppSettings::default());
    }
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("Failed to read app settings {}: {}", path.display(), error))?;
    serde_json::from_str(&content)
        .map_err(|error| format!("Failed to parse app settings {}: {}", path.display(), error))
}

fn write_app_settings(settings: &AppSettings) -> Result<(), String> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create settings directory: {}", error))?;
    }
    let content = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("Failed to serialize app settings: {}", error))?;
    fs::write(&path, content)
        .map_err(|error| format!("Failed to write app settings {}: {}", path.display(), error))
}

fn settings_path() -> PathBuf {
    config_dir().join("app_settings.json")
}
