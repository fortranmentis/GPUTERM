mod ssh;

use ssh::local_fs::{list_local_dir, load_app_settings, local_path_exists, update_recent_local_path};
use ssh::session::{
    delete_session, load_sessions, save_session, test_ssh_connection, AppState,
};
use ssh::sftp::{
    cancel_transfer, sftp_delete, sftp_download_file, sftp_list_dir, sftp_mkdir, sftp_path_exists,
    sftp_upload_file,
};
use ssh::system_monitor::{get_telemetry_settings, update_telemetry_settings};
use ssh::terminal::{
    connect_terminal, disconnect_terminal, terminal_resize, terminal_write,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            load_sessions,
            save_session,
            delete_session,
            load_app_settings,
            update_recent_local_path,
            list_local_dir,
            local_path_exists,
            test_ssh_connection,
            connect_terminal,
            terminal_write,
            terminal_resize,
            disconnect_terminal,
            get_telemetry_settings,
            update_telemetry_settings,
            sftp_list_dir,
            sftp_download_file,
            sftp_upload_file,
            sftp_path_exists,
            cancel_transfer,
            sftp_delete,
            sftp_mkdir,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run GpuTerm");
}
