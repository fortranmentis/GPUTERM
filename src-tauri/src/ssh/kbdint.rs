//! Keyboard-interactive sign-in: servers that ask questions rather than take a
//! password outright — a one-time code (Google Authenticator, Duo), or simply
//! "Password:" through PAM, which is all a stock Ubuntu sshd offers.
//!
//! A connection the user started can show the questions in a dialog
//! (`UiResponder`). Background connections (telemetry, SFTP, forwards, LLM
//! tunnels, reconnects) cannot, so `NonInteractive` answers a password
//! question with the saved password at most once and otherwise gives up with
//! a sentinel error, without guessing: each wrong answer counts as a failed
//! sign-in and can lock the account.

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// Prefix of the error a background connection returns when the server needs
/// an answer only the user can give. Format: `INTERACTIVE_AUTH_REQUIRED:{user}@{host}:{port}`.
pub const INTERACTIVE_AUTH_PREFIX: &str = "INTERACTIVE_AUTH_REQUIRED:";

/// Sent to the main window with the questions; answered by `answer_auth_prompt`.
pub const AUTH_PROMPT_EVENT: &str = "ssh-auth-prompt";

/// Below sshd's default `LoginGraceTime` (120 s), after which the server
/// hangs up regardless.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(110);

/// Words that mark a hidden prompt as asking for a code rather than the
/// account password.
const CODE_WORDS: &[&str] = &[
    "code",
    "otp",
    "token",
    "verification",
    "one-time",
    "2fa",
    "duo",
    "passcode",
    "yubikey",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// The account password (or a key passphrase).
    Password,
    /// Anything else: a one-time code, a username, a Duo choice.
    Other,
}

pub fn classify_prompt(text: &str, echo: bool) -> PromptKind {
    let lower = text.to_ascii_lowercase();
    let mentions_password = lower.contains("password") || lower.contains("passphrase");
    let mentions_code = CODE_WORDS.iter().any(|word| lower.contains(word));
    if !echo && mentions_password && !mentions_code {
        PromptKind::Password
    } else {
        PromptKind::Other
    }
}

/// `user@host:port`, the key for remembering hosts that ask for codes.
pub fn host_key(username: &str, host: &str, port: u16) -> String {
    format!("{}@{}:{}", username, host.to_ascii_lowercase(), port)
}

/// Hosts seen asking for something other than a password. Background
/// connections to them give up before contacting the server, so they never
/// spend a failed sign-in on a code they cannot know.
static CODE_HOSTS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

fn remember_code_host(key: &str) {
    if let Ok(mut hosts) = CODE_HOSTS.lock() {
        hosts.insert(key.to_string());
    }
}

pub fn asks_for_codes(key: &str) -> bool {
    CODE_HOSTS
        .lock()
        .map(|hosts| hosts.contains(key))
        .unwrap_or(false)
}

/// The hop being signed in to.
#[derive(Debug, Clone)]
pub struct HopInfo {
    pub session_id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// The saved password, offered for a password question. None when the
    /// stored secret is a key passphrase, which is not the account password.
    pub saved_password: Option<String>,
}

impl HopInfo {
    pub fn key(&self) -> String {
        host_key(&self.username, &self.host, self.port)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthAbort {
    /// Only the user can answer.
    NeedsUser,
    Canceled,
}

/// One question as libssh2 delivered it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptView {
    pub text: String,
    pub echo: bool,
}

pub trait AuthResponder: Send {
    /// Whether this responder could ever answer more than a password; used
    /// to skip keyboard-interactive entirely for known code hosts in the
    /// background.
    fn interactive(&self) -> bool;

    fn respond(
        &mut self,
        hop: &HopInfo,
        instructions: &str,
        prompts: &[PromptView],
    ) -> Result<Vec<String>, AuthAbort>;

    /// Passwords the user typed, by session id, to save once signed in.
    fn take_captured(&mut self) -> Vec<(String, String)> {
        Vec::new()
    }
}

/// For connections nobody is watching.
#[derive(Default)]
pub struct NonInteractive {
    /// Hops whose saved password was already tried: a second password
    /// question means it was wrong.
    used_password: HashSet<String>,
}

impl AuthResponder for NonInteractive {
    fn interactive(&self) -> bool {
        false
    }

    fn respond(
        &mut self,
        hop: &HopInfo,
        _instructions: &str,
        prompts: &[PromptView],
    ) -> Result<Vec<String>, AuthAbort> {
        let mut answers = Vec::with_capacity(prompts.len());
        for prompt in prompts {
            match classify_prompt(&prompt.text, prompt.echo) {
                PromptKind::Password => {
                    let Some(password) = hop.saved_password.clone() else {
                        return Err(AuthAbort::NeedsUser);
                    };
                    if !self.used_password.insert(hop.key()) {
                        return Err(AuthAbort::NeedsUser);
                    }
                    answers.push(password);
                }
                PromptKind::Other => {
                    remember_code_host(&hop.key());
                    return Err(AuthAbort::NeedsUser);
                }
            }
        }
        Ok(answers)
    }
}

/// Pending dialog answers, by request id.
pub type PendingAuthPrompts = Arc<Mutex<HashMap<String, SyncSender<Option<Vec<String>>>>>>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthPromptPayload {
    pub request_id: String,
    pub session_id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// True when this is a jump host on the way to the session's server.
    pub is_jump: bool,
    pub instructions: String,
    pub prompts: Vec<PromptView>,
}

/// For a connection the user started: asks in a dialog.
pub struct UiResponder {
    app: AppHandle,
    pending: PendingAuthPrompts,
    /// The session the user is connecting; other hops are jump hosts.
    session_id: String,
    inner: NonInteractive,
    captured: Vec<(String, String)>,
}

impl UiResponder {
    pub fn new(app: AppHandle, pending: PendingAuthPrompts, session_id: String) -> Self {
        Self {
            app,
            pending,
            session_id,
            inner: NonInteractive::default(),
            captured: Vec::new(),
        }
    }

    fn ask(
        &mut self,
        hop: &HopInfo,
        instructions: &str,
        prompts: &[PromptView],
    ) -> Result<Vec<String>, AuthAbort> {
        let request_id = uuid::Uuid::new_v4().to_string();
        let (sender, receiver) = mpsc::sync_channel(1);
        self.pending
            .lock()
            .map_err(|_| AuthAbort::Canceled)?
            .insert(request_id.clone(), sender);
        let payload = AuthPromptPayload {
            request_id: request_id.clone(),
            session_id: hop.session_id.clone(),
            host: hop.host.clone(),
            port: hop.port,
            username: hop.username.clone(),
            is_jump: hop.session_id != self.session_id,
            instructions: instructions.to_string(),
            prompts: prompts.to_vec(),
        };
        let answer = if self.app.emit_to("main", AUTH_PROMPT_EVENT, payload).is_ok() {
            receiver.recv_timeout(ANSWER_TIMEOUT).ok().flatten()
        } else {
            None
        };
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&request_id);
        }
        let answers = answer.ok_or(AuthAbort::Canceled)?;
        if answers.len() != prompts.len() {
            return Err(AuthAbort::Canceled);
        }
        Ok(answers)
    }
}

impl AuthResponder for UiResponder {
    fn interactive(&self) -> bool {
        true
    }

    fn respond(
        &mut self,
        hop: &HopInfo,
        instructions: &str,
        prompts: &[PromptView],
    ) -> Result<Vec<String>, AuthAbort> {
        // A plain password question with a saved password needs no dialog.
        if let Ok(answers) = self.inner.respond(hop, instructions, prompts) {
            return Ok(answers);
        }
        if prompts
            .iter()
            .any(|prompt| classify_prompt(&prompt.text, prompt.echo) == PromptKind::Other)
        {
            remember_code_host(&hop.key());
        }
        let answers = self.ask(hop, instructions, prompts)?;
        for (prompt, answer) in prompts.iter().zip(&answers) {
            if classify_prompt(&prompt.text, prompt.echo) == PromptKind::Password
                && !answer.is_empty()
            {
                self.captured.push((hop.session_id.clone(), answer.clone()));
            }
        }
        Ok(answers)
    }

    fn take_captured(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.captured)
    }
}

/// Bridges an `AuthResponder` to libssh2's callback, which cannot fail: an
/// abort is recorded here and the server gets empty answers.
pub struct PromptAdapter<'a> {
    pub responder: &'a mut dyn AuthResponder,
    pub hop: &'a HopInfo,
    pub aborted: Option<AuthAbort>,
}

impl ssh2::KeyboardInteractivePrompt for PromptAdapter<'_> {
    fn prompt<'b>(
        &mut self,
        _username: &str,
        instructions: &str,
        prompts: &[ssh2::Prompt<'b>],
    ) -> Vec<String> {
        // Info-only rounds (no questions) need an empty answer list.
        if prompts.is_empty() || self.aborted.is_some() {
            return vec![String::new(); prompts.len()];
        }
        let views: Vec<PromptView> = prompts
            .iter()
            .map(|prompt| PromptView {
                text: prompt.text.to_string(),
                echo: prompt.echo,
            })
            .collect();
        match self.responder.respond(self.hop, instructions, &views) {
            Ok(answers) => answers,
            Err(abort) => {
                self.aborted = Some(abort);
                vec![String::new(); prompts.len()]
            }
        }
    }
}

/// Delivers the user's answers (None for Cancel) to the waiting sign-in.
#[tauri::command]
pub fn answer_auth_prompt(
    state: tauri::State<'_, crate::ssh::session::AppState>,
    request_id: String,
    responses: Option<Vec<String>>,
) -> Result<(), String> {
    deliver_answer(&state.auth_prompts, &request_id, responses)
}

fn deliver_answer(
    pending: &PendingAuthPrompts,
    request_id: &str,
    responses: Option<Vec<String>>,
) -> Result<(), String> {
    let sender = pending
        .lock()
        .map_err(|_| "Sign-in state is unavailable".to_string())?
        .remove(request_id)
        .ok_or_else(|| "That sign-in prompt has already expired".to_string())?;
    sender
        .send(responses)
        .map_err(|_| "That sign-in prompt has already expired".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hop(saved: Option<&str>) -> HopInfo {
        HopInfo {
            session_id: "s".into(),
            host: "Lab.example".into(),
            port: 22,
            username: format!("u{}", uuid::Uuid::new_v4()),
            saved_password: saved.map(str::to_string),
        }
    }

    fn prompt(text: &str, echo: bool) -> PromptView {
        PromptView {
            text: text.into(),
            echo,
        }
    }

    #[test]
    fn tells_a_password_question_from_a_code_question() {
        assert_eq!(classify_prompt("Password: ", false), PromptKind::Password);
        assert_eq!(
            classify_prompt("alice@lab's password:", false),
            PromptKind::Password
        );
        assert_eq!(
            classify_prompt("Verification code: ", false),
            PromptKind::Other
        );
        assert_eq!(
            classify_prompt("Duo two-factor login", false),
            PromptKind::Other
        );
        assert_eq!(
            classify_prompt("One-time password (OATH)", false),
            PromptKind::Other
        );
        assert_eq!(classify_prompt("Username:", true), PromptKind::Other);
    }

    #[test]
    fn the_background_answers_a_password_question_once() {
        let hop = hop(Some("secret"));
        let mut responder = NonInteractive::default();
        assert_eq!(
            responder.respond(&hop, "", &[prompt("Password:", false)]),
            Ok(vec!["secret".to_string()])
        );
        // Asked again: the saved password was wrong; do not keep trying.
        assert_eq!(
            responder.respond(&hop, "", &[prompt("Password:", false)]),
            Err(AuthAbort::NeedsUser)
        );
    }

    #[test]
    fn the_background_never_guesses_a_code_and_remembers_the_host() {
        let hop = hop(Some("secret"));
        let mut responder = NonInteractive::default();
        assert!(!asks_for_codes(&hop.key()));
        assert_eq!(
            responder.respond(&hop, "", &[prompt("Verification code:", false)]),
            Err(AuthAbort::NeedsUser)
        );
        assert!(asks_for_codes(&hop.key()));
        // Without a saved password even a password question needs the user.
        assert_eq!(
            NonInteractive::default().respond(&self::hop(None), "", &[prompt("Password:", false)]),
            Err(AuthAbort::NeedsUser)
        );
    }

    #[test]
    fn an_answer_reaches_the_waiting_sign_in_once() {
        let pending: PendingAuthPrompts = Default::default();
        let (sender, receiver) = mpsc::sync_channel(1);
        pending.lock().unwrap().insert("r1".into(), sender);
        deliver_answer(&pending, "r1", Some(vec!["123456".into()])).unwrap();
        assert_eq!(receiver.recv().unwrap(), Some(vec!["123456".to_string()]));
        assert!(deliver_answer(&pending, "r1", None).is_err());
        assert!(deliver_answer(&pending, "unknown", None).is_err());
    }

    #[test]
    fn host_keys_ignore_host_case() {
        assert_eq!(host_key("u", "Lab.Example", 22), "u@lab.example:22");
    }
}
