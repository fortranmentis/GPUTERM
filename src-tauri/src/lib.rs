mod alerts;
mod llm;
mod native_drag;
mod ssh;

use llm::{
    delete_llm_instance, get_llm_telemetry, list_llm_instances, refresh_llm_telemetry,
    save_llm_instance, set_llm_instance_enabled, test_llm_instance,
};
use native_drag::start_native_file_drag;
use tauri::Manager;

use ssh::agent_monitor::{configure_claude_quota_monitor, refresh_agent_quota};
use ssh::local_fs::{
    describe_local_paths, list_local_dir, load_app_settings, local_path_exists,
    update_recent_local_path, update_terminal_appearance,
};
use ssh::resource_details::get_resource_details;
use ssh::session::{
    delete_session, get_credential_vault_status, has_saved_credential, initialize_credential_vault,
    load_sessions, reset_credential_vault, save_profile_forwards, save_session, test_ssh_connection, trust_host_key,
    unlock_credential_vault, AppState,
};
use ssh::sftp::{
    cancel_transfer, sftp_create_drag_out_paths, sftp_delete, sftp_download_file, sftp_list_dir,
    sftp_mkdir, sftp_path_exists, sftp_rename, sftp_upload_file,
};
use ssh::forwarding::{
    list_forwards, restart_profile_forwards, start_forward, stop_forward, validate_forward,
};
use ssh::snippets::{delete_snippet, list_snippets, save_snippet};
use ssh::ssh_config::{import_ssh_config, preview_ssh_config};
use ssh::system_monitor::{get_telemetry_settings, update_telemetry_settings};
use ssh::tmux::{kill_tmux_session, list_tmux_sessions};
use ssh::terminal::{
    connect_terminal, create_terminal_split, disconnect_terminal, disconnect_terminal_pane,
    terminal_resize, terminal_write,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            start_native_file_drag,
            load_sessions,
            save_session,
            delete_session,
            has_saved_credential,
            get_credential_vault_status,
            initialize_credential_vault,
            unlock_credential_vault,
            reset_credential_vault,
            load_app_settings,
            update_recent_local_path,
            update_terminal_appearance,
            list_local_dir,
            describe_local_paths,
            local_path_exists,
            test_ssh_connection,
            trust_host_key,
            connect_terminal,
            create_terminal_split,
            terminal_write,
            terminal_resize,
            disconnect_terminal_pane,
            disconnect_terminal,
            list_tmux_sessions,
            kill_tmux_session,
            get_telemetry_settings,
            update_telemetry_settings,
            ssh::telemetry_history::get_telemetry_history,
            alerts::get_alert_settings,
            alerts::update_alert_settings,
            refresh_agent_quota,
            configure_claude_quota_monitor,
            get_resource_details,
            sftp_list_dir,
            sftp_create_drag_out_paths,
            sftp_download_file,
            sftp_upload_file,
            sftp_path_exists,
            cancel_transfer,
            sftp_delete,
            sftp_mkdir,
            sftp_rename,
            list_snippets,
            save_snippet,
            delete_snippet,
            preview_ssh_config,
            import_ssh_config,
            save_profile_forwards,
            start_forward,
            restart_profile_forwards,
            stop_forward,
            list_forwards,
            validate_forward,
            list_llm_instances,
            save_llm_instance,
            delete_llm_instance,
            set_llm_instance_enabled,
            test_llm_instance,
            get_llm_telemetry,
            refresh_llm_telemetry,
        ])
        .setup(|app| {
            // LLM runtimes are not tied to an SSH session, so their poller
            // cannot start from `connect_terminal` like the other monitors.
            let state = app.state::<AppState>();
            // Restored before any session connects, so the first monitor
            // already polls at the interval the user chose last time.
            if let Ok(mut slot) = state.telemetry_settings.lock() {
                *slot = ssh::system_monitor::load_persisted_settings();
            }
            state.alerts.set_settings(alerts::load_persisted_settings());
            llm::load_into_state(&state);
            llm::monitor::start(app.handle().clone(), llm::monitor_context(&state));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build GpuTerm")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                app.state::<AppState>().llm_monitor.stop();
            }
        });
}
