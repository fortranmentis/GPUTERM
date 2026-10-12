//! Saved commands ("snippets") the terminal can send with one click, stored in
//! `snippets.json` next to the session profiles.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use uuid::Uuid;

use super::credentials::write_json_file;
use super::session::config_dir;

/// Serializes read-modify-write cycles over `snippets.json`.
static SNIPPETS_WRITE_LOCK: Mutex<()> = Mutex::new(());

const MAX_NAME_CHARS: usize = 80;
const MAX_COMMAND_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub command: String,
    /// Sends Return after the command, so it runs instead of waiting.
    #[serde(default)]
    pub append_return: bool,
    /// Profiles the snippet is offered for; `None` or empty means every server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_ids: Option<Vec<String>>,
}

pub fn snippets_path() -> PathBuf {
    config_dir().join("snippets.json")
}

#[tauri::command(async)]
pub fn list_snippets() -> Result<Vec<Snippet>, String> {
    read_snippets(&snippets_path())
}

#[tauri::command(async)]
pub fn save_snippet(snippet: Snippet) -> Result<Vec<Snippet>, String> {
    let snippet = normalize(snippet)?;
    let _guard = lock()?;
    let path = snippets_path();
    let snippets = upsert(read_snippets(&path)?, snippet);
    write_snippets(&path, &snippets)?;
    Ok(snippets)
}

#[tauri::command(async)]
pub fn delete_snippet(id: String) -> Result<Vec<Snippet>, String> {
    let _guard = lock()?;
    let path = snippets_path();
    let mut snippets = read_snippets(&path)?;
    snippets.retain(|snippet| snippet.id != id);
    write_snippets(&path, &snippets)?;
    Ok(snippets)
}

fn lock() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    SNIPPETS_WRITE_LOCK
        .lock()
        .map_err(|_| "Snippet storage is unavailable".to_string())
}

fn normalize(mut snippet: Snippet) -> Result<Snippet, String> {
    snippet.name = snippet.name.trim().chars().take(MAX_NAME_CHARS).collect();
    if snippet.command.trim().is_empty() {
        return Err("A snippet needs a command".to_string());
    }
    if snippet.command.len() > MAX_COMMAND_BYTES {
        return Err("A snippet can be at most 16 KB".to_string());
    }
    if snippet.name.is_empty() {
        // The first line of the command is a recognisable default.
        snippet.name = snippet
            .command
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default()
            .chars()
            .take(MAX_NAME_CHARS)
            .collect();
    }
    if snippet.id.trim().is_empty() {
        snippet.id = Uuid::new_v4().to_string();
    }
    snippet.profile_ids = snippet
        .profile_ids
        .map(|ids| {
            ids.into_iter()
                .map(|id| id.trim().to_string())
                .filter(|id| !id.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|ids| !ids.is_empty());
    Ok(snippet)
}

/// Replaces the snippet with the same id in place, or appends it.
fn upsert(mut snippets: Vec<Snippet>, snippet: Snippet) -> Vec<Snippet> {
    match snippets
        .iter_mut()
        .find(|existing| existing.id == snippet.id)
    {
        Some(existing) => *existing = snippet,
        None => snippets.push(snippet),
    }
    snippets
}

fn read_snippets(path: &Path) -> Result<Vec<Snippet>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read snippets {}: {}", path.display(), error))?;
    serde_json::from_str(&content)
        .map_err(|error| format!("Failed to parse snippets {}: {}", path.display(), error))
}

fn write_snippets(path: &Path, snippets: &[Snippet]) -> Result<(), String> {
    write_json_file(path, &snippets, "snippets file")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippet(id: &str, command: &str) -> Snippet {
        Snippet {
            id: id.to_string(),
            name: String::new(),
            command: command.to_string(),
            append_return: true,
            profile_ids: None,
        }
    }

    #[test]
    fn normalize_names_ids_and_scopes() {
        let normalized = normalize(Snippet {
            profile_ids: Some(vec![" a ".into(), "".into()]),
            ..snippet("", "\n  nvidia-smi -l 1\nexit")
        })
        .unwrap();
        assert_eq!(normalized.name, "nvidia-smi -l 1");
        assert!(!normalized.id.is_empty());
        assert_eq!(normalized.profile_ids, Some(vec!["a".to_string()]));

        let everywhere = normalize(Snippet {
            profile_ids: Some(vec!["  ".into()]),
            ..snippet("x", "ls")
        })
        .unwrap();
        assert_eq!(everywhere.profile_ids, None);

        assert!(normalize(snippet("x", "   ")).is_err());
        assert!(normalize(snippet("x", &"a".repeat(MAX_COMMAND_BYTES + 1))).is_err());
    }

    #[test]
    fn upsert_replaces_in_place_and_appends_new() {
        let list = vec![snippet("1", "a"), snippet("2", "b")];
        let list = upsert(list, snippet("1", "changed"));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].command, "changed");
        let list = upsert(list, snippet("3", "c"));
        assert_eq!(
            list.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["1", "2", "3"]
        );
    }

    #[test]
    fn storage_round_trips_and_a_missing_file_is_empty() {
        let dir = std::env::temp_dir().join(format!("gputerm-snippets-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("snippets.json");
        assert!(read_snippets(&path).unwrap().is_empty());

        let saved = vec![normalize(snippet("", "htop")).unwrap()];
        write_snippets(&path, &saved).unwrap();
        assert_eq!(read_snippets(&path).unwrap(), saved);
        // Older files without the optional fields still load.
        fs::write(&path, r#"[{"id":"x","name":"n","command":"c"}]"#).unwrap();
        let loaded = read_snippets(&path).unwrap();
        assert!(!loaded[0].append_return);
        assert_eq!(loaded[0].profile_ids, None);
        fs::remove_dir_all(dir).unwrap();
    }
}
