use crate::ssh::session::config_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use crate::ssh::system_monitor::SystemMonitorSettings;

/// Serializes read-modify-write cycles on `app_settings.json`. Every writer
/// rewrites the whole file, so two concurrent updates (say, a font change and
/// a telemetry change) would otherwise drop one of them.
static APP_SETTINGS_WRITE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_local_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_theme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_font_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_font_size: Option<u8>,
    /// Monitoring interval, display mode and ignored filesystems. Absent in
    /// files written before 1.3.4, which then start from the defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<SystemMonitorSettings>,
    /// Alert rules and notification preferences.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alerts: Option<crate::alerts::AlertSettings>,
    /// The MCP server's on/off switch and what it may expose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<crate::mcp::McpSettings>,
}

const MIN_TERMINAL_FONT_SIZE: u8 = 8;
const MAX_TERMINAL_FONT_SIZE: u8 = 28;
const MAX_APPEARANCE_ID_CHARS: usize = 64;

/// Terminal look chosen in the appearance popover. Theme and font are ids the
/// frontend resolves; an unknown id falls back to its default there.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalAppearance {
    pub theme: Option<String>,
    pub font_family: Option<String>,
    pub font_size: Option<u8>,
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

#[tauri::command(async)]
pub fn load_app_settings() -> Result<AppSettings, String> {
    read_app_settings()
}

#[tauri::command(async)]
pub fn update_recent_local_path(path: String) -> Result<AppSettings, String> {
    let normalized = validate_local_directory(&path)?;
    modify_app_settings(|settings| settings.recent_local_path = Some(normalized))
}

#[tauri::command(async)]
pub fn update_terminal_appearance(appearance: TerminalAppearance) -> Result<AppSettings, String> {
    modify_app_settings(|settings| apply_terminal_appearance(settings, appearance))
}

/// Saves the monitoring settings so they survive a restart. The caller has
/// already sanitized them.
pub(crate) fn persist_telemetry_settings(
    telemetry: &SystemMonitorSettings,
) -> Result<AppSettings, String> {
    modify_app_settings(|settings| settings.telemetry = Some(telemetry.clone()))
}

/// Reads the settings file, applies `change` and writes it back, holding the
/// write lock throughout.
pub(crate) fn modify_app_settings(
    change: impl FnOnce(&mut AppSettings),
) -> Result<AppSettings, String> {
    let _guard = APP_SETTINGS_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut settings = read_app_settings()?;
    change(&mut settings);
    write_app_settings(&settings)?;
    Ok(settings)
}

fn apply_terminal_appearance(settings: &mut AppSettings, appearance: TerminalAppearance) {
    let clean_id = |value: Option<String>| {
        value
            .map(|value| value.trim().chars().take(MAX_APPEARANCE_ID_CHARS).collect::<String>())
            .filter(|value| !value.is_empty())
    };
    settings.terminal_theme = clean_id(appearance.theme);
    settings.terminal_font_family = clean_id(appearance.font_family);
    settings.terminal_font_size = appearance
        .font_size
        .map(|size| size.clamp(MIN_TERMINAL_FONT_SIZE, MAX_TERMINAL_FONT_SIZE));
}

#[tauri::command(async)]
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

#[tauri::command(async)]
pub fn describe_local_paths(paths: Vec<String>) -> Result<Vec<LocalEntry>, String> {
    paths
        .into_iter()
        .filter(|path| !path.trim().is_empty())
        .map(|path| {
            let canonical = fs::canonicalize(path.trim()).map_err(|error| {
                format!("Local path is unavailable or does not exist: {}", error)
            })?;
            let metadata = fs::metadata(&canonical).map_err(|error| {
                format!(
                    "Failed to inspect local path {}: {}",
                    canonical.display(),
                    error
                )
            })?;
            let name = canonical
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| canonical.to_string_lossy().to_string());
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
            Ok(LocalEntry {
                name,
                path: canonical.to_string_lossy().to_string(),
                entry_type: entry_type.to_string(),
                size: metadata.is_file().then_some(metadata.len()),
                modified_time,
            })
        })
        .collect()
}

#[tauri::command(async)]
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

pub(crate) fn read_app_settings() -> Result<AppSettings, String> {
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
    crate::ssh::credentials::write_json_file(&settings_path(), settings, "app settings")
}

fn settings_path() -> PathBuf {
    config_dir().join("app_settings.json")
}

#[cfg(test)]
mod tests {
    use super::{apply_terminal_appearance, describe_local_paths, AppSettings, TerminalAppearance};
    use crate::ssh::system_monitor::SystemMonitorSettings;

    #[test]
    fn telemetry_settings_round_trip_beside_the_other_fields() {
        // Files written before telemetry was saved still load, with none set.
        let old: AppSettings =
            serde_json::from_str(r#"{"recentLocalPath":"/x","terminalFontSize":14}"#).unwrap();
        assert!(old.telemetry.is_none());

        let mut settings = old;
        settings.telemetry = Some(SystemMonitorSettings {
            telemetry_interval_secs: 5,
            display_mode: "gpu-only".to_string(),
            disk_ignore_fs_types: vec!["tmpfs".to_string()],
        });
        let json = serde_json::to_string(&settings).unwrap();
        let loaded: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.recent_local_path.as_deref(), Some("/x"));
        assert_eq!(loaded.terminal_font_size, Some(14));
        let telemetry = loaded.telemetry.clone().unwrap();
        assert_eq!(telemetry.telemetry_interval_secs, 5);
        assert_eq!(telemetry.display_mode, "gpu-only");
        assert_eq!(telemetry.disk_ignore_fs_types, vec!["tmpfs".to_string()]);

        // Changing the appearance keeps the saved telemetry settings.
        let mut settings = loaded;
        settings.telemetry = Some(telemetry);
        apply_terminal_appearance(
            &mut settings,
            TerminalAppearance {
                theme: Some("nord".to_string()),
                font_family: None,
                font_size: None,
            },
        );
        assert_eq!(settings.telemetry.unwrap().telemetry_interval_secs, 5);
    }

    #[test]
    fn terminal_appearance_is_cleaned_and_clamped() {
        let mut settings = AppSettings {
            recent_local_path: Some("/tmp".to_string()),
            ..AppSettings::default()
        };
        apply_terminal_appearance(
            &mut settings,
            TerminalAppearance {
                theme: Some(" dracula ".to_string()),
                font_family: Some("  ".to_string()),
                font_size: Some(99),
            },
        );
        assert_eq!(settings.terminal_theme.as_deref(), Some("dracula"));
        assert_eq!(settings.terminal_font_family, None);
        assert_eq!(settings.terminal_font_size, Some(28));
        assert_eq!(settings.recent_local_path.as_deref(), Some("/tmp"));

        // Older settings files without these fields still load.
        let loaded: AppSettings = serde_json::from_str(r#"{"recentLocalPath":"/x"}"#).unwrap();
        assert_eq!(loaded.terminal_font_size, None);
    }
    use std::fs;

    #[test]
    fn describes_files_pasted_from_a_local_file_manager() {
        let root = std::env::temp_dir().join(format!("gputerm-paste-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("copied file.txt");
        fs::write(&file, b"hello").unwrap();

        let entries = describe_local_paths(vec![file.to_string_lossy().to_string()]).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "copied file.txt");
        assert_eq!(entries[0].entry_type, "file");
        assert_eq!(entries[0].size, Some(5));

        fs::remove_dir_all(root).unwrap();
    }
}
