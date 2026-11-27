use super::types::ClaudeQuotaSetupResult;
use crate::ssh::system_monitor::{
    local_os, native_remote_path, remote_join, run_remote_command_for,
    write_remote_file_atomically, RemoteOs,
};
use serde_json::Value;
use ssh2::Session;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub(super) fn configure_claude_quota_local() -> Result<ClaudeQuotaSetupResult, String> {
    let os = local_os();
    if os == RemoteOs::Linux && !local_claude_helper_runtime_available(os) {
        return Ok(ClaudeQuotaSetupResult {
            status: "unsupported".to_string(),
            message: "Claude quota setup needs Python 3 on this host. Install Python 3, then run Set up again.".to_string(),
        });
    }
    let Some(home) = std::env::var_os(if os == RemoteOs::Windows {
        "USERPROFILE"
    } else {
        "HOME"
    }) else {
        return Ok(ClaudeQuotaSetupResult {
            status: "unsupported".to_string(),
            message: "Claude quota setup cannot locate this user's home directory.".to_string(),
        });
    };
    let home = PathBuf::from(home);
    let claude_dir = home.join(".claude");
    let settings_path = claude_dir.join("settings.json");
    let (helper_name, helper_contents) = claude_helper_for_os(os);
    let helper_path = claude_dir.join(helper_name);
    let desired_command =
        claude_status_line_command_for(os, helper_path.to_string_lossy().as_ref());
    fs::create_dir_all(&claude_dir)
        .map_err(|error| format!("failed to create Claude config directory: {}", error))?;
    let (mut settings, existing) = read_claude_settings(&settings_path)?;
    if let Some(command) = existing.as_deref() {
        if !command.contains("gputerm-claude-statusline") {
            return Ok(ClaudeQuotaSetupResult {
                status: "conflict".to_string(),
                message: format!(
                    "Claude already uses a custom status line (`{}`). It was not overwritten. Add {} to that status-line pipeline manually.",
                    command, desired_command
                ),
            });
        }
    }
    write_claude_helper(&helper_path, helper_contents)?;
    set_claude_status_line(&mut settings, &desired_command);
    backup_and_write_json(&settings_path, &settings)?;
    remove_stale_claude_helpers(&claude_dir, helper_name);
    Ok(ClaudeQuotaSetupResult {
        status: if existing.is_some() {
            "alreadyConfigured"
        } else {
            "configured"
        }
        .to_string(),
        message: "Claude quota monitoring is configured. Restart Claude Code, accept workspace trust if prompted, then send one message to publish the 5-hour and weekly limits.".to_string(),
    })
}

pub(super) fn configure_claude_quota_remote(
    session: &Session,
    os: RemoteOs,
) -> Result<ClaudeQuotaSetupResult, String> {
    if os == RemoteOs::Linux {
        let runtime_command = "if command -v python3 >/dev/null 2>&1 || command -v python >/dev/null 2>&1; then printf supported; else printf unsupported; fi";
        if run_remote_command_for(session, os, runtime_command)?.trim() != "supported" {
            return Ok(ClaudeQuotaSetupResult {
                status: "unsupported".to_string(),
                message: "Claude quota setup needs Python 3 on this host. Install Python 3, then run Set up again.".to_string(),
            });
        }
    }

    // Everything below moves over SFTP rather than through a shell command.
    // Embedding the helper in an exec request could not work:
    //
    // * macOS/BSD `base64` rejects a positional input file, so the POSIX
    //   decode failed and — because the shell truncates the target before
    //   running the decoder — destroyed any previously working helper.
    // * The Windows PowerShell form reached about 23,000 characters on the
    //   wire, far past cmd.exe's 8,191-character limit, and Windows OpenSSH
    //   runs exec requests through cmd.exe unless an admin changed
    //   `DefaultShell`.
    //
    // SFTP also avoids login-shell output contaminating the settings read.
    let sftp = session
        .sftp()
        .map_err(|error| format!("Claude quota setup needs SFTP on this host: {}", error))?;
    let home = sftp
        .realpath(Path::new("."))
        .map_err(|error| format!("failed to resolve the remote home directory: {}", error))?;
    let claude_dir = remote_join(&home, ".claude");
    if let Err(error) = sftp.mkdir(Path::new(&claude_dir), 0o700) {
        // Already present is the normal case; anything else is fatal.
        if sftp.stat(Path::new(&claude_dir)).is_err() {
            return Err(format!(
                "failed to create {} on the remote host: {}",
                claude_dir, error
            ));
        }
    }

    let settings_path = remote_join(&claude_dir, "settings.json");
    let (mut settings, existing) = read_remote_claude_settings(&sftp, &settings_path)?;
    let (helper_name, helper_contents) = claude_helper_for_os(os);
    let helper_path = remote_join(&claude_dir, helper_name);
    let desired_command = claude_status_line_command_for(os, &native_remote_path(&helper_path));
    if let Some(command) = existing.as_deref() {
        if !command.contains("gputerm-claude-statusline") {
            return Ok(ClaudeQuotaSetupResult {
                status: "conflict".to_string(),
                message: format!(
                    "Claude already uses a custom status line (`{}`). It was not overwritten. Add {} to that status-line pipeline manually.",
                    command, desired_command
                ),
            });
        }
    }
    set_claude_status_line(&mut settings, &desired_command);

    write_remote_file_atomically(&sftp, &helper_path, helper_contents.as_bytes())?;
    if os != RemoteOs::Windows {
        let stat = ssh2::FileStat {
            size: None,
            uid: None,
            gid: None,
            perm: Some(0o700),
            atime: None,
            mtime: None,
        };
        let _ = sftp.setstat(Path::new(&helper_path), stat);
    }

    if existing.is_some() {
        let backup = remote_join(&claude_dir, "settings.json.gputerm-backup");
        if let Ok(mut current) = sftp.open(Path::new(&settings_path)) {
            let mut bytes = Vec::new();
            if current.read_to_end(&mut bytes).is_ok() {
                let _ = write_remote_file_atomically(&sftp, &backup, &bytes);
            }
        }
    }
    let encoded = serde_json::to_vec_pretty(&settings)
        .map_err(|error| format!("failed to encode Claude settings: {}", error))?;
    write_remote_file_atomically(&sftp, &settings_path, &encoded)?;

    // A helper variant from an earlier GpuTerm release would otherwise sit
    // beside the current one, and the status line may still point at it.
    for (_, stale_name) in CLAUDE_HELPER_NAMES
        .iter()
        .filter(|(variant, _)| *variant != os)
    {
        if *stale_name != helper_name {
            let _ = sftp.unlink(Path::new(&remote_join(&claude_dir, stale_name)));
        }
    }

    Ok(ClaudeQuotaSetupResult {
        status: if existing.is_some() {
            "alreadyConfigured"
        } else {
            "configured"
        }
        .to_string(),
        message: "Claude quota monitoring is configured. Restart Claude Code, accept workspace trust if prompted, then send one message to publish the 5-hour and weekly limits.".to_string(),
    })
}

/// Every helper file name, so an install can remove the variants it replaces.
pub(super) const CLAUDE_HELPER_NAMES: [(RemoteOs, &str); 3] = [
    (RemoteOs::Windows, "gputerm-claude-statusline.ps1"),
    (RemoteOs::MacOs, "gputerm-claude-statusline-macos.js"),
    (RemoteOs::Linux, "gputerm-claude-statusline.sh"),
];

pub(super) fn read_remote_claude_settings(
    sftp: &ssh2::Sftp,
    path: &str,
) -> Result<(Value, Option<String>), String> {
    let mut file = match sftp.open(Path::new(path)) {
        Ok(file) => file,
        // Absent settings are the first-run case, not an error.
        Err(_) => return Ok((Value::Object(serde_json::Map::new()), None)),
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read Claude settings: {}", error))?;
    let settings = parse_claude_settings(&bytes)?;
    let command = claude_status_line_command(&settings);
    Ok((settings, command))
}

/// Parses `settings.json`, tolerating an empty file and a UTF-8 BOM. Windows
/// editors and PowerShell redirection both write the BOM, and `serde_json`
/// treats it as a syntax error.
pub(super) fn parse_claude_settings(bytes: &[u8]) -> Result<Value, String> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    serde_json::from_str::<Value>(text)
        .map_err(|error| format!("Claude settings.json is not valid JSON: {}", error))
}

pub(super) fn local_claude_helper_runtime_available(os: RemoteOs) -> bool {
    if matches!(os, RemoteOs::Windows | RemoteOs::MacOs) {
        return true;
    }
    let candidates: &[&str] = &["python3", "python"];
    candidates.iter().any(|candidate| {
        let mut command = Command::new(candidate);
        command
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        command.status().is_ok_and(|status| status.success())
    })
}

pub(super) fn read_claude_settings(path: &Path) -> Result<(Value, Option<String>), String> {
    let settings = match fs::read(path) {
        Ok(bytes) => parse_claude_settings(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Value::Object(serde_json::Map::new())
        }
        Err(error) => return Err(format!("failed to read Claude settings: {}", error)),
    };
    let command = claude_status_line_command(&settings);
    Ok((settings, command))
}

pub(super) fn claude_status_line_command(settings: &Value) -> Option<String> {
    settings
        .pointer("/statusLine/command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(str::to_string)
}

pub(super) fn set_claude_status_line(settings: &mut Value, command: &str) {
    if !settings.is_object() {
        *settings = Value::Object(serde_json::Map::new());
    }
    settings.as_object_mut().unwrap().insert(
        "statusLine".to_string(),
        serde_json::json!({
            "type": "command",
            "command": command,
            "padding": 0,
            "refreshInterval": 30
        }),
    );
}

pub(super) fn claude_helper_for_os(os: RemoteOs) -> (&'static str, &'static str) {
    match os {
        RemoteOs::Windows => (
            "gputerm-claude-statusline.ps1",
            include_str!("../../../../scripts/gputerm-claude-statusline.ps1"),
        ),
        RemoteOs::MacOs => (
            "gputerm-claude-statusline-macos.js",
            include_str!("../../../../scripts/gputerm-claude-statusline-macos.js"),
        ),
        RemoteOs::Linux => (
            "gputerm-claude-statusline.sh",
            include_str!("../../../../scripts/gputerm-claude-statusline.sh"),
        ),
    }
}

pub(super) fn claude_status_line_command_for(os: RemoteOs, helper_path: &str) -> String {
    match os {
        RemoteOs::Windows => {
            let helper_path = helper_path.replace('\\', "/").replace('"', "\\\"");
            format!(
                "powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\"",
                helper_path
            )
        }
        RemoteOs::MacOs => {
            "/usr/bin/osascript -l JavaScript \"$HOME/.claude/gputerm-claude-statusline-macos.js\""
                .to_string()
        }
        RemoteOs::Linux => "~/.claude/gputerm-claude-statusline.sh".to_string(),
    }
}

/// Installs the helper through a temporary sibling and a rename, so a failure
/// partway through cannot leave an empty file where a working helper was.
pub(super) fn write_claude_helper(path: &Path, contents: &str) -> Result<(), String> {
    let temporary = path.with_extension("gputerm-new");
    fs::write(&temporary, contents)
        .map_err(|error| format!("failed to install Claude status-line helper: {}", error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("failed to make Claude helper executable: {}", error))?;
    }
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("failed to install Claude status-line helper: {}", error)
    })
}

/// Removes helper variants this OS no longer uses, so a stale file cannot sit
/// beside the current one with the status line still pointing at it.
pub(super) fn remove_stale_claude_helpers(claude_dir: &Path, keep: &str) {
    for (_, name) in CLAUDE_HELPER_NAMES.iter().filter(|(_, name)| *name != keep) {
        let _ = fs::remove_file(claude_dir.join(name));
    }
}

pub(super) fn backup_and_write_json(path: &Path, value: &Value) -> Result<(), String> {
    if path.exists() {
        let backup = path.with_file_name("settings.json.gputerm-backup");
        fs::copy(path, backup)
            .map_err(|error| format!("failed to back up Claude settings: {}", error))?;
    }
    let temporary = path.with_file_name("settings.json.gputerm-new");
    let encoded = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("failed to encode Claude settings: {}", error))?;
    fs::write(&temporary, encoded)
        .map_err(|error| format!("failed to write Claude settings: {}", error))?;
    fs::rename(&temporary, path)
        .map_err(|error| format!("failed to replace Claude settings: {}", error))
}
