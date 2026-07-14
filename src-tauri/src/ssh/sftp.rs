use crate::ssh::session::{
    open_ssh_session, target_for_active_session, with_ops_session, AppState, SshTarget,
};
use serde::{Deserialize, Serialize};
use ssh2::FileStat;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

const OPS_TIMEOUT_MS: u32 = 10_000;
const DOWNLOAD_PART_SUFFIX: &str = ".gputerm-part";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpEntry {
    name: String,
    path: String,
    #[serde(rename = "type")]
    entry_type: String,
    size: Option<u64>,
    modified_time: Option<u64>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpListResponse {
    path: String,
    entries: Vec<SftpEntry>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpTransferRequest {
    session_id: String,
    remote_path: String,
    local_path: String,
    transfer_id: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpPathRequest {
    session_id: String,
    remote_path: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SftpProgressPayload {
    transfer_id: Option<String>,
    session_id: String,
    operation: String,
    remote_path: String,
    local_path: String,
    transferred_bytes: u64,
    total_bytes: Option<u64>,
    done: bool,
    error: Option<String>,
}

#[tauri::command]
pub async fn sftp_list_dir(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
) -> Result<SftpListResponse, String> {
    let target = target_for_active_session(&state, &session_id)?;
    let ops = Arc::clone(&state.ops_sessions);
    tauri::async_runtime::spawn_blocking(move || {
        with_ops_session(&ops, &target, OPS_TIMEOUT_MS, |session| {
            let sftp = session
                .sftp()
                .map_err(|error| format!("SFTP failed to start: {}", error))?;
            let requested = if path.trim().is_empty() { "." } else { path.trim() };
            let remote_path = Path::new(requested);
            let real_path = sftp
                .realpath(remote_path)
                .unwrap_or_else(|_| PathBuf::from(requested));
            let mut entries = sftp
                .readdir(remote_path)
                .map_err(|error| {
                    format!("SFTP directory listing failed for {}: {}", requested, error)
                })?
                .into_iter()
                .filter_map(|(path, stat)| sftp_entry(path, stat))
                .collect::<Vec<_>>();

            entries.sort_by(|a, b| {
                let left_dir = a.entry_type == "directory";
                let right_dir = b.entry_type == "directory";
                right_dir
                    .cmp(&left_dir)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });

            Ok(SftpListResponse {
                path: real_path.to_string_lossy().to_string(),
                entries,
            })
        })
    })
    .await
    .map_err(|error| format!("SFTP list task failed: {}", error))?
}

#[tauri::command]
pub async fn sftp_download_file(
    app: AppHandle,
    state: State<'_, AppState>,
    request: SftpTransferRequest,
) -> Result<(), String> {
    let target = target_for_active_session(&state, &request.session_id)?;
    let cancel = register_transfer_cancel(&state, request.transfer_id.as_deref());
    let transfer_id = request.transfer_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        download_file(app, target, request, cancel.as_deref())
    })
    .await
    .map_err(|error| format!("SFTP download task failed: {}", error));
    remove_transfer_cancel(&state, transfer_id.as_deref());
    result?
}

#[tauri::command]
pub async fn sftp_upload_file(
    app: AppHandle,
    state: State<'_, AppState>,
    request: SftpTransferRequest,
) -> Result<(), String> {
    let target = target_for_active_session(&state, &request.session_id)?;
    let cancel = register_transfer_cancel(&state, request.transfer_id.as_deref());
    let transfer_id = request.transfer_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        upload_file(app, target, request, cancel.as_deref())
    })
    .await
    .map_err(|error| format!("SFTP upload task failed: {}", error));
    remove_transfer_cancel(&state, transfer_id.as_deref());
    result?
}

#[tauri::command]
pub async fn sftp_delete(
    state: State<'_, AppState>,
    request: SftpPathRequest,
) -> Result<(), String> {
    let target = target_for_active_session(&state, &request.session_id)?;
    let ops = Arc::clone(&state.ops_sessions);
    tauri::async_runtime::spawn_blocking(move || {
        with_ops_session(&ops, &target, OPS_TIMEOUT_MS, |session| {
            let sftp = session
                .sftp()
                .map_err(|error| format!("SFTP failed to start: {}", error))?;
            let remote_path = Path::new(&request.remote_path);
            let stat = sftp.stat(remote_path).map_err(|error| {
                format!("SFTP stat failed for {}: {}", request.remote_path, error)
            })?;

            if file_type(&stat) == "directory" {
                sftp.rmdir(remote_path)
                    .map_err(|error| format!("SFTP directory delete failed: {}", error))
            } else {
                sftp.unlink(remote_path)
                    .map_err(|error| format!("SFTP file delete failed: {}", error))
            }
        })
    })
    .await
    .map_err(|error| format!("SFTP delete task failed: {}", error))?
}

#[tauri::command]
pub async fn sftp_mkdir(
    state: State<'_, AppState>,
    request: SftpPathRequest,
) -> Result<(), String> {
    let target = target_for_active_session(&state, &request.session_id)?;
    let ops = Arc::clone(&state.ops_sessions);
    tauri::async_runtime::spawn_blocking(move || {
        with_ops_session(&ops, &target, OPS_TIMEOUT_MS, |session| {
            let sftp = session
                .sftp()
                .map_err(|error| format!("SFTP failed to start: {}", error))?;
            sftp.mkdir(Path::new(&request.remote_path), 0o755).map_err(|error| {
                format!("SFTP mkdir failed for {}: {}", request.remote_path, error)
            })
        })
    })
    .await
    .map_err(|error| format!("SFTP mkdir task failed: {}", error))?
}

#[tauri::command]
pub async fn sftp_path_exists(
    state: State<'_, AppState>,
    request: SftpPathRequest,
) -> Result<bool, String> {
    let target = target_for_active_session(&state, &request.session_id)?;
    let ops = Arc::clone(&state.ops_sessions);
    tauri::async_runtime::spawn_blocking(move || {
        with_ops_session(&ops, &target, OPS_TIMEOUT_MS, |session| {
            let sftp = session
                .sftp()
                .map_err(|error| format!("SFTP failed to start: {}", error))?;
            Ok(sftp.stat(Path::new(&request.remote_path)).is_ok())
        })
    })
    .await
    .map_err(|error| format!("SFTP stat task failed: {}", error))?
}

#[tauri::command]
pub fn cancel_transfer(state: State<AppState>, transfer_id: String) -> Result<(), String> {
    if let Ok(cancels) = state.transfer_cancels.lock() {
        if let Some(flag) = cancels.get(&transfer_id) {
            flag.store(true, Ordering::SeqCst);
        }
    }
    // A missing flag means the transfer already finished; treat as success.
    Ok(())
}

fn register_transfer_cancel(
    state: &AppState,
    transfer_id: Option<&str>,
) -> Option<Arc<AtomicBool>> {
    let transfer_id = transfer_id?;
    let flag = Arc::new(AtomicBool::new(false));
    if let Ok(mut cancels) = state.transfer_cancels.lock() {
        cancels.insert(transfer_id.to_string(), Arc::clone(&flag));
    }
    Some(flag)
}

fn remove_transfer_cancel(state: &AppState, transfer_id: Option<&str>) {
    if let Some(transfer_id) = transfer_id {
        if let Ok(mut cancels) = state.transfer_cancels.lock() {
            cancels.remove(transfer_id);
        }
    }
}

fn download_file(
    app: AppHandle,
    target: SshTarget,
    request: SftpTransferRequest,
    cancel: Option<&AtomicBool>,
) -> Result<(), String> {
    let session = open_ssh_session(&target)?;
    let sftp = session
        .sftp()
        .map_err(|error| format!("SFTP failed to start: {}", error))?;
    let remote_path = Path::new(&request.remote_path);
    let total_bytes = sftp.stat(remote_path).ok().and_then(|stat| stat.size);
    let mut remote_file = sftp
        .open(remote_path)
        .map_err(|error| format!("SFTP download open failed: {}", error))?;

    let local_path = Path::new(&request.local_path);
    if let Some(parent) = local_path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create local directory {}: {}", parent.display(), error))?;
    }

    // Write to a temp file so failed or canceled downloads never leave a
    // partial file at the final path.
    let part_path = PathBuf::from(format!("{}{}", request.local_path, DOWNLOAD_PART_SUFFIX));
    let mut local_file = fs::File::create(&part_path)
        .map_err(|error| format!("Failed to create local file {}: {}", part_path.display(), error))?;

    let result = transfer_read_to_write(
        &mut remote_file,
        &mut local_file,
        &app,
        &request,
        "download",
        total_bytes,
        cancel,
    );
    drop(local_file);

    if result.is_err() {
        let _ = fs::remove_file(&part_path);
        return result;
    }

    if local_path.exists() {
        // Overwrite was confirmed by the user before the transfer started;
        // Windows rename fails when the destination exists.
        fs::remove_file(local_path)
            .map_err(|error| format!("Failed to replace {}: {}", request.local_path, error))?;
    }
    fs::rename(&part_path, local_path)
        .map_err(|error| format!("Failed to finalize {}: {}", request.local_path, error))
}

fn upload_file(
    app: AppHandle,
    target: SshTarget,
    request: SftpTransferRequest,
    cancel: Option<&AtomicBool>,
) -> Result<(), String> {
    let total_bytes = fs::metadata(&request.local_path)
        .map_err(|error| format!("Failed to inspect local file {}: {}", request.local_path, error))?
        .len();
    let mut local_file = fs::File::open(&request.local_path)
        .map_err(|error| format!("Failed to open local file {}: {}", request.local_path, error))?;

    let session = open_ssh_session(&target)?;
    let sftp = session
        .sftp()
        .map_err(|error| format!("SFTP failed to start: {}", error))?;
    let mut remote_file = sftp
        .create(Path::new(&request.remote_path))
        .map_err(|error| format!("SFTP upload create failed: {}", error))?;

    transfer_read_to_write(
        &mut local_file,
        &mut remote_file,
        &app,
        &request,
        "upload",
        Some(total_bytes),
        cancel,
    )
}

fn transfer_read_to_write<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    app: &AppHandle,
    request: &SftpTransferRequest,
    operation: &str,
    total_bytes: Option<u64>,
    cancel: Option<&AtomicBool>,
) -> Result<(), String> {
    const CHUNK_SIZE: usize = 1024 * 1024;
    let mut buffer = [0_u8; CHUNK_SIZE];
    let mut transferred_bytes = 0_u64;
    emit_progress(app, request, operation, transferred_bytes, total_bytes, false, None);

    loop {
        if cancel.map(|flag| flag.load(Ordering::SeqCst)).unwrap_or(false) {
            let message = "Transfer canceled".to_string();
            emit_progress(
                app,
                request,
                operation,
                transferred_bytes,
                total_bytes,
                true,
                Some(message.clone()),
            );
            return Err(message);
        }

        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => {
                let message = format!("SFTP {} read failed: {}", operation, error);
                emit_progress(
                    app,
                    request,
                    operation,
                    transferred_bytes,
                    total_bytes,
                    true,
                    Some(message.clone()),
                );
                return Err(message);
            }
        };

        if let Err(error) = writer.write_all(&buffer[..read]) {
            let message = format!("SFTP {} write failed: {}", operation, error);
            emit_progress(
                app,
                request,
                operation,
                transferred_bytes,
                total_bytes,
                true,
                Some(message.clone()),
            );
            return Err(message);
        }

        transferred_bytes += read as u64;
        emit_progress(app, request, operation, transferred_bytes, total_bytes, false, None);
    }

    writer
        .flush()
        .map_err(|error| format!("SFTP {} flush failed: {}", operation, error))?;
    emit_progress(app, request, operation, transferred_bytes, total_bytes, true, None);
    Ok(())
}

fn emit_progress(
    app: &AppHandle,
    request: &SftpTransferRequest,
    operation: &str,
    transferred_bytes: u64,
    total_bytes: Option<u64>,
    done: bool,
    error: Option<String>,
) {
    let _ = app.emit(
        "sftp-progress",
        SftpProgressPayload {
            transfer_id: request.transfer_id.clone(),
            session_id: request.session_id.clone(),
            operation: operation.to_string(),
            remote_path: request.remote_path.clone(),
            local_path: request.local_path.clone(),
            transferred_bytes,
            total_bytes,
            done,
            error,
        },
    );
}

fn sftp_entry(path: PathBuf, stat: FileStat) -> Option<SftpEntry> {
    let name = path.file_name()?.to_string_lossy().to_string();
    if name == "." || name == ".." {
        return None;
    }

    Some(SftpEntry {
        name,
        path: path.to_string_lossy().to_string(),
        entry_type: file_type(&stat).to_string(),
        size: stat.size,
        modified_time: stat.mtime,
    })
}

fn file_type(stat: &FileStat) -> &'static str {
    const S_IFMT: u32 = 0o170000;
    const S_IFDIR: u32 = 0o040000;
    const S_IFREG: u32 = 0o100000;
    const S_IFLNK: u32 = 0o120000;

    match stat.perm.map(|perm| perm & S_IFMT) {
        Some(S_IFDIR) => "directory",
        Some(S_IFREG) => "file",
        Some(S_IFLNK) => "symlink",
        _ => "other",
    }
}
