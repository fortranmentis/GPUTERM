//! Read-only monitoring for self-hosted LLM runtimes (Ollama and vLLM).
//!
//! Every request originates here, in the backend: the webview's CSP forbids it
//! from reaching an external host, and the API key never leaves this process.
//! Nothing in this module sends an inference request — monitoring must not
//! perturb, or bill, the server it is watching.

pub mod adapter;
pub mod http;
pub mod instance;
pub mod monitor;
pub mod ollama;
pub mod prometheus;
pub mod severity;
pub mod tunnel;
pub mod vllm;

use std::sync::Arc;
use std::time::Duration;
use tauri::State;

use crate::ssh::credentials::CredentialStore;
use crate::ssh::session::{list_profiles, AppState};
use adapter::RuntimeStatus;
use http::UreqClient;
use instance::{
    read_instances, sanitize_instance, upsert, vault_key, write_instances, LlmInstance,
};
use monitor::{now_epoch_seconds, LlmTelemetryPayload};

/// Loads the registered instances into shared state at startup.
pub fn load_into_state(state: &AppState) {
    match read_instances() {
        Ok(instances) => {
            if let Ok(mut slot) = state.llm_instances.lock() {
                *slot = instances;
            }
        }
        Err(error) => {
            // A malformed file must not stop the app from starting; the list
            // stays empty and the user can re-register.
            eprintln!("GpuTerm: could not load LLM instances: {error}");
        }
    }
}

/// Writes the list to disk and swaps it into shared state, so the poller picks
/// the change up on its next tick.
fn persist(state: &AppState, instances: Vec<LlmInstance>) -> Result<Vec<LlmInstance>, String> {
    write_instances(&instances)?;
    if let Ok(mut slot) = state.llm_instances.lock() {
        *slot = instances.clone();
    }
    state.llm_monitor.request_refresh();
    Ok(instances)
}

/// Reads the registered list from disk, so a file that failed to parse at
/// startup is reported to the user instead of looking like an empty list.
#[tauri::command]
pub async fn list_llm_instances(state: State<'_, AppState>) -> Result<Vec<LlmInstance>, String> {
    let instances = tauri::async_runtime::spawn_blocking(read_instances)
        .await
        .map_err(|error| format!("Loading LLM instances failed: {}", error))??;
    if let Ok(mut slot) = state.llm_instances.lock() {
        *slot = instances.clone();
    }
    Ok(instances)
}

/// Adds or updates one instance.
///
/// `api_key` is a separate argument rather than a field on `LlmInstance`: it
/// goes straight to the encrypted vault and is never written to the config file
/// or returned to the caller. `Some("")` clears a stored key; `None` leaves an
/// existing one untouched, so an edit does not have to re-send the secret.
#[tauri::command]
pub async fn save_llm_instance(
    state: State<'_, AppState>,
    instance: LlmInstance,
    api_key: Option<String>,
) -> Result<Vec<LlmInstance>, String> {
    let credentials = state.credentials.clone();

    let (instances, key_error) = tauri::async_runtime::spawn_blocking(move || {
        // Read from disk rather than the in-memory copy: if the file failed to
        // parse at startup that copy is empty, and writing it back would delete
        // every other instance. An unreadable file must fail the save loudly.
        let mut instances = read_instances()?;
        let now = now_epoch_seconds();
        let id = instance.id.clone();
        // Validated here rather than in `sanitize_instance`, which stays IO-free
        // so its unit tests need no profile file.
        validate_tunnel_profile(instance.ssh_profile_id.as_deref())?;
        upsert(&mut instances, instance, now)?;

        let key_error = match api_key {
            Some(key) if key.trim().is_empty() => credentials.clear_password(&vault_key(&id)).err(),
            Some(key) => credentials.set_password(&vault_key(&id), key).err(),
            None => None,
        };
        Ok::<_, String>((instances, key_error))
    })
    .await
    .map_err(|error| format!("Saving the LLM instance failed: {}", error))??;

    let saved = persist(&state, instances)?;
    // The instance itself is saved either way; a locked vault only means the key
    // could not be stored, and the user needs to be told which happened.
    if let Some(error) = key_error {
        return Err(format!("The instance was saved, but the API key was not: {error}"));
    }
    Ok(saved)
}

#[tauri::command]
pub async fn delete_llm_instance(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<LlmInstance>, String> {
    let credentials = state.credentials.clone();
    let mut instances = read_instances()?;
    instances.retain(|instance| instance.id != id);

    // Best effort, matching profile deletion: a locked vault must not make
    // removal impossible, and the orphaned entry is unreachable regardless.
    if let Err(error) = credentials.clear_password(&vault_key(&id)) {
        eprintln!("GpuTerm: could not clear the stored API key for a deleted LLM instance: {error}");
    }
    persist(&state, instances)
}

#[tauri::command]
pub async fn set_llm_instance_enabled(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<Vec<LlmInstance>, String> {
    let mut instances = read_instances()?;
    let Some(instance) = instances.iter_mut().find(|instance| instance.id == id) else {
        return Err("That LLM instance no longer exists".to_string());
    };
    instance.enabled = enabled;
    instance.updated_at = now_epoch_seconds();
    persist(&state, instances)
}

/// Rejects a tunnel profile that cannot serve as one, with the specific reason.
fn validate_tunnel_profile(profile_id: Option<&str>) -> Result<(), String> {
    let Some(profile_id) = profile_id else {
        return Ok(());
    };
    let profile = list_profiles()?
        .into_iter()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| "That SSH session profile no longer exists".to_string())?;
    if profile.is_local {
        return Err("A local terminal profile cannot be used as a tunnel".to_string());
    }
    Ok(())
}

/// Checks an address once without saving it, so the form can verify before the
/// user commits. Uses the supplied key if there is one, otherwise the stored
/// key for an instance being edited.
///
/// For a tunneled instance this is also the trust bootstrap: it propagates the
/// untrusted-host-key sentinel so the form can show the fingerprint prompt, which
/// the background poller has no way to do.
#[tauri::command]
pub async fn test_llm_instance(
    state: State<'_, AppState>,
    instance: LlmInstance,
    api_key: Option<String>,
) -> Result<RuntimeStatus, String> {
    let instance = sanitize_instance(instance)?;
    validate_tunnel_profile(instance.ssh_profile_id.as_deref())?;
    let credentials = state.credentials.clone();
    let active = Arc::clone(&state.active_connections);

    tauri::async_runtime::spawn_blocking(move || {
        let key = match api_key {
            Some(key) if !key.trim().is_empty() => Some(key),
            // Fall back to the saved key so testing an edit does not require
            // re-typing the secret.
            _ => monitor::credential_for(&instance, &credentials)
                .map_err(|error| error.message)?,
        };

        // Held for the whole test; dropping it tears the tunnel down. Kept
        // deliberately separate from the poller's own connections.
        let mut _tunnel = None;
        let endpoint = match instance.ssh_profile_id.as_deref() {
            None => instance.base_url.clone(),
            Some(profile_id) => {
                let (url, guard) =
                    tunnel::open_one_shot(&active, &credentials, profile_id, &instance.base_url)?;
                _tunnel = Some(guard);
                url
            }
        };

        let client = UreqClient::new(
            &endpoint,
            key,
            Duration::from_millis(instance.request_timeout_ms),
        );
        let now = now_epoch_seconds();
        Ok(monitor::adapter_for(&instance, now).check_health(&client))
    })
    .await
    .map_err(|error| format!("The connection test failed to run: {}", error))?
}

/// The latest poll result, so a freshly opened popover has data immediately
/// instead of waiting for the next tick.
#[tauri::command]
pub async fn get_llm_telemetry(
    state: State<'_, AppState>,
) -> Result<Option<LlmTelemetryPayload>, String> {
    Ok(state.llm_monitor.snapshot())
}

/// Asks the poller to re-check every enabled instance now.
#[tauri::command]
pub async fn refresh_llm_telemetry(state: State<'_, AppState>) -> Result<(), String> {
    state.llm_monitor.request_refresh();
    Ok(())
}

/// Builds the context the coordinator thread needs from managed state.
pub fn monitor_context(state: &AppState) -> monitor::MonitorContext {
    monitor::MonitorContext {
        instances: Arc::clone(&state.llm_instances),
        credentials: state.credentials.clone(),
        handle: Arc::clone(&state.llm_monitor),
        active_connections: Arc::clone(&state.active_connections),
    }
}
