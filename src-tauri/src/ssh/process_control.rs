//! Ending a process from the CPU, memory and GPU process tables.
//!
//! A PID alone is not a safe target: by the time the user confirms, the
//! process may have exited and its PID been reused. So the dialog first
//! inspects the process and reads a *start marker* (when it started), and the
//! signal is sent by one remote script that re-reads the marker and refuses if
//! it changed. Nothing here elevates privileges: a process the SSH user does
//! not own reports "permission denied".

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;

use crate::ssh::parse_util::split_sections;
use crate::ssh::resource_details::resolve_remote_os;
use crate::ssh::session::{target_for_active_session, with_ops_session, AppState};
use crate::ssh::system_monitor::{
    local_os, run_local_command_for, run_remote_command_for, run_remote_command_with_budget,
    shell_quote, RemoteOs, WINDOWS_COMMAND_TIMEOUT_MS,
};

const PROCESS_TIMEOUT_MS: u32 = 5_000;
const PROCESS_BUDGET_SECS: u64 = 5;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessIdentity {
    pub pid: u32,
    /// False when no such process is visible to the SSH user.
    pub alive: bool,
    pub user: Option<String>,
    pub command: Option<String>,
    /// When the process started, in a form only compared for equality.
    pub start_marker: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessSignal {
    /// Ask the process to exit (SIGTERM; on Windows, a close request).
    Term,
    /// Force it (SIGKILL; on Windows, `Stop-Process -Force`).
    Kill,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalOutcome {
    /// sent | gone | identity_changed | permission_denied | needs_force | failed
    pub status: &'static str,
    pub message: Option<String>,
}

impl SignalOutcome {
    fn new(status: &'static str, message: Option<String>) -> Self {
        Self { status, message }
    }
}

/// PIDs that must never be signalled: the kernel's idle task and init, and,
/// on this computer, GpuTerm itself.
fn validate_pid(pid: u32, is_local: bool) -> Result<(), String> {
    if pid <= 1 {
        return Err(format!("Process {pid} cannot be ended from GpuTerm"));
    }
    if is_local && pid == std::process::id() {
        return Err("That is GpuTerm itself".to_string());
    }
    Ok(())
}

/// Markers are digits (Linux, Windows) or an `lstart` date (macOS); anything
/// else is refused before it gets near a shell.
fn validate_marker(marker: &str) -> Result<(), String> {
    let valid = !marker.is_empty()
        && marker.len() <= 64
        && marker
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == ':');
    if valid {
        Ok(())
    } else {
        Err("Invalid process start marker".to_string())
    }
}

/// Prints the start marker: field 22 of /proc/<pid>/stat (start time in clock
/// ticks; counted after the last `)` because the command name may contain
/// spaces and parentheses), or `ps -o lstart` where there is no /proc.
const POSIX_MARKER_FN: &str = "marker() { if [ -r /proc/$1/stat ]; then sed 's/^.*) //' /proc/$1/stat 2>/dev/null | cut -d' ' -f20; else LC_ALL=C ps -o lstart= -p \"$1\" 2>/dev/null | sed 's/^ *//;s/ *$//'; fi; }";

pub(crate) fn posix_inspect_script(pid: u32) -> String {
    format!(
        "{POSIX_MARKER_FN}\n\
         printf '__MARKER__\\n'; marker {pid}\n\
         printf '__USER__\\n'; ps -o user= -p {pid} 2>/dev/null || stat -c %U /proc/{pid} 2>/dev/null\n\
         printf '__CMD__\\n'; ps -o args= -p {pid} 2>/dev/null || tr '\\0' ' ' < /proc/{pid}/cmdline 2>/dev/null\n\
         true"
    )
}

/// Every path prints one token and exits 0, so the outcome is read from
/// stdout rather than lost in a generic non-zero-exit error.
pub(crate) fn posix_signal_script(pid: u32, marker: &str, signal: ProcessSignal) -> String {
    let name = match signal {
        ProcessSignal::Term => "TERM",
        ProcessSignal::Kill => "KILL",
    };
    let want = shell_quote(marker);
    format!(
        "{POSIX_MARKER_FN}\n\
         cur=$(marker {pid})\n\
         [ -n \"$cur\" ] || {{ echo __GONE__; exit 0; }}\n\
         [ \"$cur\" = {want} ] || {{ echo __CHANGED__; exit 0; }}\n\
         if err=$(kill -s {name} {pid} 2>&1); then echo __SENT__; else echo \"__FAILED__ $err\"; fi"
    )
}

const WINDOWS_MARKER: &str = "$p.CreationDate.ToUniversalTime().Ticks.ToString()";

pub(crate) fn windows_inspect_script(pid: u32) -> String {
    format!(
        "$p = Get-CimInstance Win32_Process -Filter \"ProcessId={pid}\"\n\
         if (-not $p) {{ '{{}}'; exit 0 }}\n\
         $o = Invoke-CimMethod -InputObject $p -MethodName GetOwner -ErrorAction SilentlyContinue\n\
         $user = if ($o -and $o.User) {{ if ($o.Domain) {{ \"$($o.Domain)\\$($o.User)\" }} else {{ $o.User }} }} else {{ $null }}\n\
         [pscustomobject]@{{ marker = {WINDOWS_MARKER}; user = $user; command = if ($p.CommandLine) {{ $p.CommandLine }} else {{ $p.Name }} }} | ConvertTo-Json -Compress"
    )
}

pub(crate) fn windows_signal_script(pid: u32, marker: &str, signal: ProcessSignal) -> String {
    // `taskkill` without /F asks the process to close, the nearest thing to
    // SIGTERM; console programs refuse it, which comes back as needs_force.
    let action = match signal {
        ProcessSignal::Term => format!(
            "$out = & taskkill.exe /PID {pid} 2>&1 | Out-String\n\
             if ($LASTEXITCODE -eq 0) {{ '__SENT__' }} else {{ \"__FAILED__ $out\" }}"
        ),
        ProcessSignal::Kill => format!(
            "try {{ Stop-Process -Id {pid} -Force -ErrorAction Stop; '__SENT__' }} catch {{ \"__FAILED__ $($_.Exception.Message)\" }}"
        ),
    };
    format!(
        "$p = Get-CimInstance Win32_Process -Filter \"ProcessId={pid}\"\n\
         if (-not $p) {{ '__GONE__'; exit 0 }}\n\
         if ({WINDOWS_MARKER} -ne '{marker}') {{ '__CHANGED__'; exit 0 }}\n\
         {action}"
    )
}

fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

pub(crate) fn parse_inspect_output(os: RemoteOs, pid: u32, output: &str) -> ProcessIdentity {
    let (start_marker, user, command) = if os == RemoteOs::Windows {
        let value: serde_json::Value = serde_json::from_str(output.trim()).unwrap_or_default();
        let field = |key: &str| value.get(key).and_then(|v| v.as_str()).and_then(non_empty);
        (field("marker"), field("user"), field("command"))
    } else {
        let sections = split_sections(output);
        let section = |key: &str| sections.get(key).and_then(|text| non_empty(text));
        (section("MARKER"), section("USER"), section("CMD"))
    };
    ProcessIdentity {
        pid,
        alive: start_marker.is_some(),
        user,
        command,
        start_marker,
    }
}

pub(crate) fn parse_signal_output(output: &str) -> SignalOutcome {
    let line = output
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("__"))
        .unwrap_or("");
    match line {
        "__SENT__" => SignalOutcome::new("sent", None),
        "__GONE__" => SignalOutcome::new("gone", None),
        "__CHANGED__" => SignalOutcome::new("identity_changed", None),
        _ => {
            // Everything after the token: Windows puts the reason on the
            // next line.
            let detail = output
                .split_once("__FAILED__")
                .map_or(output, |(_, rest)| rest)
                .trim()
                .to_string();
            let lower = detail.to_ascii_lowercase();
            if lower.contains("not permitted") || lower.contains("access is denied") {
                SignalOutcome::new(
                    "permission_denied",
                    Some(
                        "This process belongs to another user. GpuTerm does not use sudo to end it."
                            .to_string(),
                    ),
                )
            } else if lower.contains("no such process") || lower.contains("not found") {
                SignalOutcome::new("gone", None)
            } else if lower.contains("forcefully") {
                SignalOutcome::new("needs_force", Some(detail))
            } else {
                SignalOutcome::new("failed", non_empty(&detail))
            }
        }
    }
}

/// Runs `script` (built per OS by `build`) on the session's host.
async fn run_on_host(
    state: &State<'_, AppState>,
    session_id: &str,
    pid: u32,
    build: impl FnOnce(RemoteOs) -> String + Send + 'static,
) -> Result<(RemoteOs, String), String> {
    let is_local = state
        .active_connections
        .lock()
        .map_err(|_| "Active session state is unavailable".to_string())?
        .get(session_id)
        .map(|connection| connection.profile.is_local)
        .ok_or_else(|| "No active terminal session is available".to_string())?;
    validate_pid(pid, is_local)?;

    if is_local {
        let os = local_os();
        return tauri::async_runtime::spawn_blocking(move || {
            run_local_command_for(os, &build(os)).map(|output| (os, output))
        })
        .await
        .map_err(|error| format!("Process task failed: {}", error))?;
    }

    let target = target_for_active_session(state, session_id)?;
    let ops = Arc::clone(&state.ops_sessions);
    let os_cache = Arc::clone(&state.remote_os_cache);
    tauri::async_runtime::spawn_blocking(move || {
        with_ops_session(&ops, &target, PROCESS_TIMEOUT_MS, |session| {
            let os = resolve_remote_os(&os_cache, &target.session_id, session);
            let script = build(os);
            let output = if os == RemoteOs::Windows {
                session.set_timeout(WINDOWS_COMMAND_TIMEOUT_MS);
                run_remote_command_for(session, os, &script)
            } else {
                run_remote_command_with_budget(session, &script, PROCESS_BUDGET_SECS)
            }?;
            Ok((os, output))
        })
    })
    .await
    .map_err(|error| format!("Process task failed: {}", error))?
}

/// Who owns a process, what it runs, and when it started.
#[tauri::command]
pub async fn inspect_process(
    state: State<'_, AppState>,
    session_id: String,
    pid: u32,
) -> Result<ProcessIdentity, String> {
    let (os, output) = run_on_host(&state, &session_id, pid, move |os| {
        if os == RemoteOs::Windows {
            windows_inspect_script(pid)
        } else {
            posix_inspect_script(pid)
        }
    })
    .await?;
    Ok(parse_inspect_output(os, pid, &output))
}

/// Signals a process, but only if it is still the one that was inspected.
#[tauri::command]
pub async fn signal_process(
    state: State<'_, AppState>,
    session_id: String,
    pid: u32,
    signal: ProcessSignal,
    expected_start_marker: String,
) -> Result<SignalOutcome, String> {
    validate_marker(&expected_start_marker)?;
    let (_, output) = run_on_host(&state, &session_id, pid, move |os| {
        if os == RemoteOs::Windows {
            windows_signal_script(pid, &expected_start_marker, signal)
        } else {
            posix_signal_script(pid, &expected_start_marker, signal)
        }
    })
    .await?;
    Ok(parse_signal_output(&output))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_pids_that_must_never_be_signalled() {
        assert!(validate_pid(0, false).is_err());
        assert!(validate_pid(1, false).is_err());
        assert!(validate_pid(std::process::id(), true).is_err());
        assert!(validate_pid(std::process::id(), false).is_ok());
        assert!(validate_pid(4242, true).is_ok());
    }

    #[test]
    fn markers_cannot_carry_shell_syntax() {
        assert!(validate_marker("123456").is_ok());
        assert!(validate_marker("Mon Oct  6 10:01:02 2026").is_ok());
        for bad in ["", "1'; rm -rf ~", "$(id)", "a\nb", &"9".repeat(65)] {
            assert!(validate_marker(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_signal_script_rechecks_the_marker_before_signalling() {
        let script = posix_signal_script(4242, "Mon Oct  6 10:01:02 2026", ProcessSignal::Term);
        assert!(script.contains("cur=$(marker 4242)"));
        assert!(script.contains("[ \"$cur\" = 'Mon Oct  6 10:01:02 2026' ]"));
        assert!(script.contains("kill -s TERM 4242"));
        let marker_check = script.find("__CHANGED__").unwrap();
        assert!(marker_check < script.find("kill -s").unwrap());
        assert!(posix_signal_script(4242, "1", ProcessSignal::Kill).contains("kill -s KILL 4242"));

        let windows = windows_signal_script(4242, "638000000000000000", ProcessSignal::Kill);
        assert!(windows.contains("-ne '638000000000000000'"));
        assert!(windows.contains("Stop-Process -Id 4242 -Force"));
        assert!(windows_signal_script(4242, "1", ProcessSignal::Term)
            .contains("taskkill.exe /PID 4242"));
    }

    #[test]
    fn the_stat_marker_survives_a_command_name_with_parentheses() {
        // Fields after the last ")" begin at field 3, so field 22 is the
        // 20th: here 987654.
        let stat = "4242 (my (weird) proc) S 1 4242 4242 0 -1 4194560 100 0 0 0 5 3 0 0 20 0 1 0 987654 1000 10";
        let after = &stat[stat.rfind(") ").unwrap() + 2..];
        assert_eq!(after.split(' ').nth(19), Some("987654"));
        assert!(POSIX_MARKER_FN.contains("sed 's/^.*) //'"));
        assert!(POSIX_MARKER_FN.contains("cut -d' ' -f20"));
    }

    #[test]
    fn parses_posix_inspection() {
        let output = "__MARKER__\n987654\n__USER__\nalice\n__CMD__\npython train.py --epochs 3\n";
        assert_eq!(
            parse_inspect_output(RemoteOs::Linux, 4242, output),
            ProcessIdentity {
                pid: 4242,
                alive: true,
                user: Some("alice".into()),
                command: Some("python train.py --epochs 3".into()),
                start_marker: Some("987654".into()),
            }
        );
        let gone = parse_inspect_output(RemoteOs::MacOs, 4242, "__MARKER__\n__USER__\n__CMD__\n");
        assert!(!gone.alive);
        assert_eq!(gone.user, None);
    }

    #[test]
    fn parses_windows_inspection() {
        let output = r#"{"marker":"638000000000000000","user":"LAB\\alice","command":"python.exe train.py"}"#;
        let identity = parse_inspect_output(RemoteOs::Windows, 4242, output);
        assert!(identity.alive);
        assert_eq!(identity.user.as_deref(), Some("LAB\\alice"));
        assert!(!parse_inspect_output(RemoteOs::Windows, 4242, "{}").alive);
    }

    #[test]
    fn maps_signal_results() {
        assert_eq!(parse_signal_output("__SENT__\n").status, "sent");
        assert_eq!(parse_signal_output("__GONE__").status, "gone");
        assert_eq!(
            parse_signal_output("__CHANGED__").status,
            "identity_changed"
        );
        assert_eq!(
            parse_signal_output("__FAILED__ sh: kill: (4242) - Operation not permitted").status,
            "permission_denied"
        );
        assert_eq!(
            parse_signal_output("__FAILED__ kill: No such process").status,
            "gone"
        );
        assert_eq!(
            parse_signal_output("__FAILED__ ERROR: Access is denied.").status,
            "permission_denied"
        );
        let force = parse_signal_output(
            "__FAILED__ ERROR: The process with PID 4242 could not be terminated.\r\nReason: This process can only be terminated forcefully (with /F option).",
        );
        assert_eq!(force.status, "needs_force");
        let other = parse_signal_output("__FAILED__ something odd");
        assert_eq!(
            (other.status, other.message.as_deref()),
            ("failed", Some("something odd"))
        );
    }
}
