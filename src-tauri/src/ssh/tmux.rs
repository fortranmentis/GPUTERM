//! tmux session discovery and control for the terminal toolbar's tmux picker.
//!
//! Listing runs over the pooled ops connection like the other request/response
//! collectors. Attaching and creating deliberately do **not** live here: the
//! frontend opens a split pane and types the tmux command into that pane's
//! login shell, so tmux inherits a real interactive environment and detaching
//! drops the user back to a prompt instead of closing the pane.

use crate::ssh::resource_details::resolve_remote_os;
use crate::ssh::session::{target_for_active_session, with_ops_session, AppState};
use crate::ssh::system_monitor::{
    local_os, run_local_command_for, run_remote_command_with_budget, RemoteOs,
};
use serde::Serialize;
use std::sync::Arc;
use tauri::State;

/// Generous compared with `DETAIL_TIMEOUT_MS`: `with_ops_session` spends one
/// round trip on its liveness probe, the exec costs another, and `sh -l`
/// sources `/etc/profile` and `~/.profile` before tmux even starts. Through a
/// ProxyJump to a loaded login node, three seconds is marginal.
const TMUX_TIMEOUT_MS: u32 = 8_000;
const TMUX_BUDGET_SECS: u64 = 5;

/// `sh -l` sources `/etc/profile` and `~/.profile` — but not `~/.bashrc`,
/// `~/.zshrc`, or `~/.zprofile`, which is exactly where Homebrew on Apple
/// Silicon puts `/opt/homebrew/bin`. So probe the usual absolute locations
/// after PATH, the same way `GPU_PROBE_COMMAND` does for the GPU tools.
const TMUX_PATHS: &str =
    "tmux /usr/local/bin/tmux /opt/homebrew/bin/tmux \"$HOME/.local/bin/tmux\" /snap/bin/tmux";

/// Always exits 0 so the caller never sees tmux's own exit status: `tmux
/// list-sessions` exits 1 with "no server running on ..." whenever the server
/// is down, which is the single most common state and emphatically not an
/// error. Absence of tmux is reported by the absence of the `B` line instead.
///
/// Rows are prefixed with a literal tag rather than the repo's `__SECTION__`
/// markers on purpose: a session named `__PROC_STAT__` would otherwise open a
/// bogus section and truncate the list. Anything that is not a recognised tag
/// is ignored, so a name containing a newline can corrupt only its own row.
///
/// `session_name` comes last because it is the only free-form field — tmux
/// forbids `.` and `:` in names but permits spaces, tabs and quotes.
fn tmux_list_command() -> String {
    format!(
        "for c in {paths}; do if command -v \"$c\" >/dev/null 2>&1; then printf 'B\\t%s\\n' \"$c\"; \"$c\" list-sessions -F \"S\t#{{session_id}}\t#{{session_windows}}\t#{{session_attached}}\t#{{session_created}}\t#{{session_name}}\" 2>/dev/null; break; fi; done; exit 0",
        paths = TMUX_PATHS
    )
}

/// `exec`s tmux so its exit status propagates: unlike listing, a failed kill
/// must surface as an error. `id` is validated as `$<digits>` beforehand, so
/// single-quoting it is safe in every login shell.
fn tmux_kill_command(id: &str) -> String {
    format!(
        "for c in {paths}; do if command -v \"$c\" >/dev/null 2>&1; then exec \"$c\" kill-session -t '{id}'; fi; done; echo 'tmux is not available on this host' >&2; exit 127",
        paths = TMUX_PATHS,
        id = id
    )
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TmuxStatus {
    /// tmux was not found on PATH or in the probed locations.
    #[default]
    NotInstalled,
    Ok,
    /// Windows hosts: there is no tmux, and `sh -lc` would not reach PowerShell.
    UnsupportedOs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TmuxSession {
    /// tmux's own handle (`$0`, `$1`, ...). Stable, free of shell metacharacters,
    /// and the only value ever interpolated into a command.
    pub id: String,
    /// Display only. Never interpolated into a shell command.
    pub name: String,
    pub windows: u32,
    pub attached: bool,
    /// Unix seconds. tmux < 2.2 emits a date string here, which stays `None`
    /// rather than dropping an otherwise-usable row.
    pub created: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TmuxListing {
    pub status: TmuxStatus,
    pub sessions: Vec<TmuxSession>,
}

impl TmuxListing {
    fn unsupported() -> Self {
        Self {
            status: TmuxStatus::UnsupportedOs,
            sessions: Vec::new(),
        }
    }
}

/// True for tmux session ids only (`$` followed by digits). Everything written
/// into a live shell must pass this first.
pub(crate) fn is_tmux_session_id(value: &str) -> bool {
    match value.strip_prefix('$') {
        Some(digits) => !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()),
        None => false,
    }
}

pub(crate) fn parse_tmux_listing(output: &str) -> TmuxListing {
    let mut listing = TmuxListing::default();
    for line in output.lines() {
        if line.starts_with("B\t") {
            listing.status = TmuxStatus::Ok;
        } else if let Some(row) = line.strip_prefix("S\t") {
            if let Some(session) = parse_tmux_session_row(row) {
                listing.sessions.push(session);
            }
        }
    }
    listing
}

fn parse_tmux_session_row(row: &str) -> Option<TmuxSession> {
    // The name takes the remainder, so a name containing tabs stays intact.
    let mut fields = row.splitn(5, '\t');
    let id = fields.next()?.trim().to_string();
    let windows = fields.next()?.trim().parse().unwrap_or(0);
    let attached = fields.next()?.trim().parse::<u32>().unwrap_or(0) > 0;
    let created = fields.next()?.trim().parse::<i64>().ok();
    let name = fields.next()?.to_string();
    if !is_tmux_session_id(&id) || name.is_empty() {
        return None;
    }
    Some(TmuxSession {
        id,
        name,
        windows,
        attached,
        created,
    })
}

#[tauri::command]
pub async fn list_tmux_sessions(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<TmuxListing, String> {
    // Must precede `target_for_active_session`, which hard-errors for local
    // profiles — and a local macOS shell very often does have tmux.
    let is_local = state
        .active_connections
        .lock()
        .map_err(|_| "Active session state is unavailable".to_string())?
        .get(&session_id)
        .map(|connection| connection.profile.is_local)
        .ok_or_else(|| "No active terminal session is available".to_string())?;

    if is_local {
        let os = local_os();
        if os == RemoteOs::Windows {
            return Ok(TmuxListing::unsupported());
        }
        return tauri::async_runtime::spawn_blocking(move || {
            run_local_command_for(os, &tmux_list_command())
                .map(|output| parse_tmux_listing(&output))
        })
        .await
        .map_err(|error| format!("tmux listing task failed: {}", error))?;
    }

    let target = target_for_active_session(&state, &session_id)?;
    let ops = Arc::clone(&state.ops_sessions);
    let os_cache = Arc::clone(&state.remote_os_cache);
    tauri::async_runtime::spawn_blocking(move || {
        with_ops_session(&ops, &target, TMUX_TIMEOUT_MS, |session| {
            // Note: `classify_uname` maps MSYS/Cygwin to Windows, so such a
            // host is reported unsupported even if it does ship tmux.
            if resolve_remote_os(&os_cache, &target.session_id, session) == RemoteOs::Windows {
                return Ok(TmuxListing::unsupported());
            }
            run_remote_command_with_budget(session, &tmux_list_command(), TMUX_BUDGET_SECS)
                .map(|output| parse_tmux_listing(&output))
        })
    })
    .await
    .map_err(|error| format!("tmux listing task failed: {}", error))?
}

#[tauri::command]
pub async fn kill_tmux_session(
    state: State<'_, AppState>,
    session_id: String,
    tmux_id: String,
) -> Result<(), String> {
    if !is_tmux_session_id(&tmux_id) {
        return Err("Invalid tmux session id".to_string());
    }

    let is_local = state
        .active_connections
        .lock()
        .map_err(|_| "Active session state is unavailable".to_string())?
        .get(&session_id)
        .map(|connection| connection.profile.is_local)
        .ok_or_else(|| "No active terminal session is available".to_string())?;

    let command = tmux_kill_command(&tmux_id);

    if is_local {
        let os = local_os();
        if os == RemoteOs::Windows {
            return Err("tmux is not available on this host".to_string());
        }
        return tauri::async_runtime::spawn_blocking(move || {
            run_local_command_for(os, &command).map(|_| ())
        })
        .await
        .map_err(|error| format!("tmux kill task failed: {}", error))?;
    }

    let target = target_for_active_session(&state, &session_id)?;
    let ops = Arc::clone(&state.ops_sessions);
    tauri::async_runtime::spawn_blocking(move || {
        with_ops_session(&ops, &target, TMUX_TIMEOUT_MS, |session| {
            run_remote_command_with_budget(session, &command, TMUX_BUDGET_SECS).map(|_| ())
        })
    })
    .await
    .map_err(|error| format!("tmux kill task failed: {}", error))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sessions_with_window_counts_and_attach_state() {
        let listing = parse_tmux_listing(
            "B\t/usr/bin/tmux\nS\t$0\t3\t0\t1700000000\ttrain\nS\t$1\t1\t2\t1700000100\teval\n",
        );
        assert_eq!(listing.status, TmuxStatus::Ok);
        assert_eq!(listing.sessions.len(), 2);
        assert_eq!(listing.sessions[0].id, "$0");
        assert_eq!(listing.sessions[0].name, "train");
        assert_eq!(listing.sessions[0].windows, 3);
        assert!(!listing.sessions[0].attached);
        assert_eq!(listing.sessions[0].created, Some(1_700_000_000));
        // `session_attached` is a client count, not a boolean.
        assert!(listing.sessions[1].attached);
    }

    #[test]
    fn treats_a_running_tmux_with_no_sessions_as_ok_and_empty() {
        let listing = parse_tmux_listing("B\t/usr/bin/tmux\n");
        assert_eq!(listing.status, TmuxStatus::Ok);
        assert!(listing.sessions.is_empty());
    }

    #[test]
    fn reports_not_installed_when_the_probe_found_nothing() {
        let listing = parse_tmux_listing("");
        assert_eq!(listing.status, TmuxStatus::NotInstalled);
        assert!(listing.sessions.is_empty());
    }

    #[test]
    fn keeps_names_containing_spaces_quotes_and_tabs() {
        let listing =
            parse_tmux_listing("B\t/usr/bin/tmux\nS\t$4\t1\t0\t1700000000\tmy it's\tsession\n");
        assert_eq!(listing.sessions.len(), 1);
        assert_eq!(listing.sessions[0].name, "my it's\tsession");
    }

    #[test]
    fn keeps_rows_whose_created_field_is_not_an_epoch() {
        // tmux < 2.2 formats session_created as a date string.
        let listing =
            parse_tmux_listing("B\t/usr/bin/tmux\nS\t$0\t1\t0\tWed Nov 15 10:00:00 2023\told\n");
        assert_eq!(listing.sessions.len(), 1);
        assert_eq!(listing.sessions[0].created, None);
    }

    #[test]
    fn skips_rows_with_missing_fields_and_untagged_lines() {
        let listing = parse_tmux_listing(
            "B\t/usr/bin/tmux\nS\t$0\t2\nno server running on /tmp/tmux-1000/default\nS\t$1\t1\t0\t1700000000\tok\n",
        );
        assert_eq!(listing.sessions.len(), 1);
        assert_eq!(listing.sessions[0].name, "ok");
    }

    #[test]
    fn ignores_a_session_name_that_imitates_a_section_marker() {
        let listing = parse_tmux_listing(
            "B\t/usr/bin/tmux\nS\t$0\t1\t0\t1700000000\t__PROC_STAT__\nS\t$1\t1\t0\t1700000000\treal\n",
        );
        // The impostor is a normal row, and crucially does not truncate the list.
        assert_eq!(listing.sessions.len(), 2);
        assert_eq!(listing.sessions[1].name, "real");
    }

    #[test]
    fn rejects_rows_whose_id_is_not_a_tmux_session_id() {
        let listing = parse_tmux_listing("B\t/usr/bin/tmux\nS\tevil\t1\t0\t1700000000\tx\n");
        assert!(listing.sessions.is_empty());
    }

    #[test]
    fn validates_tmux_session_ids() {
        assert!(is_tmux_session_id("$0"));
        assert!(is_tmux_session_id("$42"));
        assert!(!is_tmux_session_id("$"));
        assert!(!is_tmux_session_id("0"));
        assert!(!is_tmux_session_id("$1a"));
        assert!(!is_tmux_session_id("$1; rm -rf /"));
        assert!(!is_tmux_session_id(""));
    }

    #[test]
    fn kill_command_only_ever_embeds_a_validated_id() {
        let command = tmux_kill_command("$3");
        assert!(command.contains("kill-session -t '$3'"));
    }
}
