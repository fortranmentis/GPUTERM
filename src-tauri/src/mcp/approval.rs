//! Asks the user, in the main window, whether an MCP client may run an
//! action. Same request/answer shape as the keyboard-interactive dialog in
//! `ssh/kbdint.rs`: the tool call blocks on a channel the UI answers.

use std::collections::HashMap;
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use super::McpState;

pub const APPROVAL_EVENT: &str = "mcp-approval-request";
/// Tells the UI to drop a request that timed out unanswered.
pub const APPROVAL_CLOSED_EVENT: &str = "mcp-approval-closed";
/// Under common MCP client request timeouts, so a timeout is reported as a
/// denial rather than a dropped request.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(110);

pub type PendingApprovals = Arc<Mutex<HashMap<String, SyncSender<bool>>>>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub request_id: String,
    /// The client's self-reported name (from `initialize`).
    pub client: String,
    pub tool: String,
    pub host: String,
    /// One line: what will happen.
    pub summary: String,
    /// The exact command, path or process, shown verbatim.
    pub detail: String,
}

impl ApprovalRequest {
    pub fn new(client: &str, tool: &str, host: &str, summary: String, detail: String) -> Self {
        Self {
            request_id: uuid::Uuid::new_v4().to_string(),
            client: client.to_string(),
            tool: tool.to_string(),
            host: host.to_string(),
            summary,
            detail,
        }
    }
}

/// Shows the request and waits for the answer. No answer within the timeout,
/// or no window to ask in, is a denial.
pub fn request_approval(app: &AppHandle, request: ApprovalRequest) -> bool {
    let pending = Arc::clone(&app.state::<McpState>().approvals);
    let (sender, receiver) = mpsc::sync_channel(1);
    let request_id = request.request_id.clone();
    match pending.lock() {
        Ok(mut pending) => {
            pending.insert(request_id.clone(), sender);
        }
        Err(_) => return false,
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    let approved = if app.emit_to("main", APPROVAL_EVENT, request).is_ok() {
        receiver.recv_timeout(ANSWER_TIMEOUT).unwrap_or(false)
    } else {
        false
    };
    let unanswered = pending
        .lock()
        .map(|mut pending| pending.remove(&request_id).is_some())
        .unwrap_or(false);
    if unanswered {
        let _ = app.emit_to("main", APPROVAL_CLOSED_EVENT, &request_id);
    }
    approved
}

/// The user's answer to an approval request.
#[tauri::command]
pub fn answer_mcp_approval(
    state: tauri::State<'_, McpState>,
    request_id: String,
    approved: bool,
) -> Result<(), String> {
    deliver(&state.approvals, &request_id, approved)
}

fn deliver(pending: &PendingApprovals, request_id: &str, approved: bool) -> Result<(), String> {
    let sender = pending
        .lock()
        .map_err(|_| "Approval state is unavailable".to_string())?
        .remove(request_id)
        .ok_or_else(|| "That request has already expired".to_string())?;
    sender
        .send(approved)
        .map_err(|_| "That request has already expired".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_reaches_the_waiting_call_once() {
        let pending = PendingApprovals::default();
        let (sender, receiver) = mpsc::sync_channel(1);
        pending.lock().unwrap().insert("r1".into(), sender);

        deliver(&pending, "r1", true).unwrap();
        assert_eq!(receiver.recv_timeout(Duration::from_millis(10)), Ok(true));
        assert!(deliver(&pending, "r1", false).is_err());
    }

    #[test]
    fn an_unknown_or_abandoned_request_is_refused() {
        let pending = PendingApprovals::default();
        assert!(deliver(&pending, "missing", true).is_err());

        let (sender, receiver) = mpsc::sync_channel(1);
        pending.lock().unwrap().insert("r2".into(), sender);
        drop(receiver);
        assert!(deliver(&pending, "r2", true).is_err());
    }
}
