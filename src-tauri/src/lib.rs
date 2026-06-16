mod ssh;

use ssh::session::{
    delete_session, load_sessions, save_session, test_ssh_connection, AppState,
};
use ssh::sftp::{
    sftp_delete, sftp_download_file, sftp_list_dir, sftp_mkdir, sftp_upload_file,
};
use ssh::system_monitor::{get_telemetry_settings, update_telemetry_settings};
use ssh::terminal::{
    connect_terminal, disconnect_terminal, terminal_resize, terminal_write,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            load_sessions,
            save_session,
            delete_session,
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
            sftp_delete,
            sftp_mkdir,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run GpuTerm");
}
