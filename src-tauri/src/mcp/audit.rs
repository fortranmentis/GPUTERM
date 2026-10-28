//! An append-only record of every MCP tool call, in `mcp-audit.log` next to
//! the other GpuTerm files: one JSON object per line.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::ssh::session::config_dir;
use crate::ssh::system_monitor::now_epoch_seconds;

/// Past this size the log moves to `mcp-audit.log.1`, replacing the old one.
const MAX_LOG_BYTES: u64 = 1024 * 1024;

static WRITE_LOCK: Mutex<()> = Mutex::new(());

pub fn log_path() -> PathBuf {
    config_dir().join("mcp-audit.log")
}

/// Records one call. `outcome` is "ok", "error" or "denied". Arguments are
/// kept as sent (a command's text, a path); nothing a tool returns is.
pub fn record(client: &str, tool: &str, arguments: &Value, outcome: &str, error: Option<&str>) {
    let entry = json!({
        "at": now_epoch_seconds(),
        "client": client,
        "tool": tool,
        "arguments": arguments,
        "outcome": outcome,
        "error": error,
    });
    let _guard = WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _ = append(&log_path(), &entry.to_string());
}

fn append(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::metadata(path)
        .map(|meta| meta.len() >= MAX_LOG_BYTES)
        .unwrap_or(false)
    {
        let mut rotated = path.as_os_str().to_owned();
        rotated.push(".1");
        fs::rename(path, PathBuf::from(rotated))?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    writeln!(file, "{line}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_lines_and_rotates_a_full_log() {
        let dir = std::env::temp_dir().join(format!("gputerm-audit-{}", uuid::Uuid::new_v4()));
        let path = dir.join("mcp-audit.log");
        append(&path, "first").unwrap();
        append(&path, "second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first\nsecond\n");

        fs::write(&path, vec![b'x'; MAX_LOG_BYTES as usize]).unwrap();
        append(&path, "third").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "third\n");
        assert_eq!(
            fs::metadata(dir.join("mcp-audit.log.1")).unwrap().len(),
            MAX_LOG_BYTES
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
