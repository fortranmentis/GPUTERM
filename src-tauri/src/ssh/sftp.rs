use crate::ssh::session::{
    open_ssh_session, target_for_active_session, with_ops_session, AppState, SshTarget,
};
use serde::{Deserialize, Serialize};
use ssh2::{FileStat, Sftp};
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter, State};

const OPS_TIMEOUT_MS: u32 = 10_000;
const DOWNLOAD_PART_SUFFIX: &str = ".gputerm-part";
const DRAG_OUT_CACHE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

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
pub struct SftpDragOutPath {
    remote_path: String,
    local_path: String,
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
            let requested = if path.trim().is_empty() {
                "."
            } else {
                path.trim()
            };
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

/// Reserves absolute, application-owned temporary paths for native OS drag-out.
///
/// Finder, Explorer and Linux file managers can only receive real local paths;
/// remote SFTP paths are materialized into these paths before the native drag
/// operation starts. Old exports are retained long enough for file managers to
/// finish copying and are pruned on later exports.
#[tauri::command]
pub fn sftp_create_drag_out_paths(
    remote_paths: Vec<String>,
) -> Result<Vec<SftpDragOutPath>, String> {
    if remote_paths.is_empty() {
        return Err("Select at least one remote item to drag".to_string());
    }
    if remote_paths.len() > 256 {
        return Err("Too many remote items selected for one drag".to_string());
    }

    let cache_root = std::env::temp_dir().join("gputerm").join("drag-out");
    fs::create_dir_all(&cache_root).map_err(|error| {
        format!(
            "Failed to create SFTP drag cache {}: {}",
            cache_root.display(),
            error
        )
    })?;
    cleanup_drag_out_cache(&cache_root);

    let export_dir = cache_root.join(uuid::Uuid::new_v4().to_string());
    fs::create_dir(&export_dir).map_err(|error| {
        format!(
            "Failed to create SFTP drag export {}: {}",
            export_dir.display(),
            error
        )
    })?;

    reserve_drag_out_paths(&export_dir, remote_paths).inspect_err(|_| {
        let _ = fs::remove_dir_all(&export_dir);
    })
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
        download_path(app, target, request, cancel.as_deref())
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
        upload_path(app, target, request, cancel.as_deref())
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
            let stat = sftp.lstat(remote_path).map_err(|error| {
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
            sftp.mkdir(Path::new(&request.remote_path), 0o755)
                .map_err(|error| {
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

fn download_path(
    app: AppHandle,
    target: SshTarget,
    request: SftpTransferRequest,
    cancel: Option<&AtomicBool>,
) -> Result<(), String> {
    let connection = open_ssh_session(&target)?;
    let sftp = connection
        .session
        .sftp()
        .map_err(|error| format!("SFTP failed to start: {}", error))?;
    let remote_path = Path::new(&request.remote_path);
    let remote_stat = sftp
        .lstat(remote_path)
        .map_err(|error| format!("SFTP stat failed for {}: {}", request.remote_path, error))?;
    let total_bytes = remote_entry_size(&sftp, remote_path, &remote_stat)?;
    let mut transferred_bytes = 0;
    emit_progress(
        &app,
        &request,
        "download",
        transferred_bytes,
        Some(total_bytes),
        false,
        None,
    );

    let result = download_remote_entry(
        &sftp,
        remote_path,
        &remote_stat,
        Path::new(&request.local_path),
        &app,
        &request,
        Some(total_bytes),
        cancel,
        &mut transferred_bytes,
    );
    finish_transfer(
        &app,
        &request,
        "download",
        transferred_bytes,
        Some(total_bytes),
        result,
    )
}

fn upload_path(
    app: AppHandle,
    target: SshTarget,
    request: SftpTransferRequest,
    cancel: Option<&AtomicBool>,
) -> Result<(), String> {
    let local_path = Path::new(&request.local_path);
    let total_bytes = local_entry_size(local_path)?;

    let connection = open_ssh_session(&target)?;
    let sftp = connection
        .session
        .sftp()
        .map_err(|error| format!("SFTP failed to start: {}", error))?;
    let mut transferred_bytes = 0;
    emit_progress(
        &app,
        &request,
        "upload",
        transferred_bytes,
        Some(total_bytes),
        false,
        None,
    );
    let result = upload_local_entry(
        &sftp,
        local_path,
        Path::new(&request.remote_path),
        &app,
        &request,
        Some(total_bytes),
        cancel,
        &mut transferred_bytes,
    );
    finish_transfer(
        &app,
        &request,
        "upload",
        transferred_bytes,
        Some(total_bytes),
        result,
    )
}

#[allow(clippy::too_many_arguments)]
fn upload_local_entry(
    sftp: &Sftp,
    local_path: &Path,
    remote_path: &Path,
    app: &AppHandle,
    request: &SftpTransferRequest,
    total_bytes: Option<u64>,
    cancel: Option<&AtomicBool>,
    transferred_bytes: &mut u64,
) -> Result<(), String> {
    check_transfer_canceled(cancel)?;
    let metadata = fs::symlink_metadata(local_path).map_err(|error| {
        format!(
            "Failed to inspect local path {}: {}",
            local_path.display(),
            error
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "Symbolic links are not supported for folder transfer: {}",
            local_path.display()
        ));
    }

    if metadata.is_dir() {
        ensure_remote_directory(sftp, remote_path)?;
        let entries = fs::read_dir(local_path).map_err(|error| {
            format!(
                "Failed to read local directory {}: {}",
                local_path.display(),
                error
            )
        })?;
        for entry in entries {
            check_transfer_canceled(cancel)?;
            let entry = entry
                .map_err(|error| format!("Failed to read local directory entry: {}", error))?;
            let child_remote = remote_child_path(remote_path, &entry.file_name());
            upload_local_entry(
                sftp,
                &entry.path(),
                &child_remote,
                app,
                request,
                total_bytes,
                cancel,
                transferred_bytes,
            )?;
        }
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(format!("Unsupported local entry: {}", local_path.display()));
    }

    if let Ok(stat) = sftp.lstat(remote_path) {
        if file_type(&stat) == "directory" {
            remove_remote_entry(sftp, remote_path, &stat)?;
        }
    }
    let mut local_file = fs::File::open(local_path).map_err(|error| {
        format!(
            "Failed to open local file {}: {}",
            local_path.display(),
            error
        )
    })?;
    let mut remote_file = sftp.create(remote_path).map_err(|error| {
        format!(
            "SFTP upload create failed for {}: {}",
            remote_path.display(),
            error
        )
    })?;
    copy_transfer_stream(
        &mut local_file,
        &mut remote_file,
        app,
        request,
        "upload",
        total_bytes,
        cancel,
        transferred_bytes,
    )
}

#[allow(clippy::too_many_arguments)]
fn download_remote_entry(
    sftp: &Sftp,
    remote_path: &Path,
    remote_stat: &FileStat,
    local_path: &Path,
    app: &AppHandle,
    request: &SftpTransferRequest,
    total_bytes: Option<u64>,
    cancel: Option<&AtomicBool>,
    transferred_bytes: &mut u64,
) -> Result<(), String> {
    check_transfer_canceled(cancel)?;
    match file_type(remote_stat) {
        "directory" => {
            ensure_local_directory(local_path)?;
            for (child_path, child_stat) in remote_directory_entries(sftp, remote_path)? {
                check_transfer_canceled(cancel)?;
                let child_name = child_path.file_name().ok_or_else(|| {
                    format!("Remote entry has no file name: {}", child_path.display())
                })?;
                download_remote_entry(
                    sftp,
                    &child_path,
                    &child_stat,
                    &local_path.join(child_name),
                    app,
                    request,
                    total_bytes,
                    cancel,
                    transferred_bytes,
                )?;
            }
            Ok(())
        }
        "file" => download_remote_file(
            sftp,
            remote_path,
            local_path,
            app,
            request,
            total_bytes,
            cancel,
            transferred_bytes,
        ),
        "symlink" => Err(format!(
            "Symbolic links are not supported for folder transfer: {}",
            remote_path.display()
        )),
        _ => Err(format!(
            "Unsupported remote entry: {}",
            remote_path.display()
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn download_remote_file(
    sftp: &Sftp,
    remote_path: &Path,
    local_path: &Path,
    app: &AppHandle,
    request: &SftpTransferRequest,
    total_bytes: Option<u64>,
    cancel: Option<&AtomicBool>,
    transferred_bytes: &mut u64,
) -> Result<(), String> {
    if let Some(parent) = local_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create local directory {}: {}",
                parent.display(),
                error
            )
        })?;
    }

    let mut remote_file = sftp.open(remote_path).map_err(|error| {
        format!(
            "SFTP download open failed for {}: {}",
            remote_path.display(),
            error
        )
    })?;
    let part_path = PathBuf::from(format!("{}{}", local_path.display(), DOWNLOAD_PART_SUFFIX));
    remove_local_entry_if_exists(&part_path)?;
    let mut local_file = fs::File::create(&part_path).map_err(|error| {
        format!(
            "Failed to create local file {}: {}",
            part_path.display(),
            error
        )
    })?;
    let result = copy_transfer_stream(
        &mut remote_file,
        &mut local_file,
        app,
        request,
        "download",
        total_bytes,
        cancel,
        transferred_bytes,
    );
    drop(local_file);
    if let Err(error) = result {
        let _ = fs::remove_file(&part_path);
        return Err(error);
    }

    remove_local_entry_if_exists(local_path)?;
    fs::rename(&part_path, local_path)
        .map_err(|error| format!("Failed to finalize {}: {}", local_path.display(), error))
}

#[allow(clippy::too_many_arguments)]
fn copy_transfer_stream<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    app: &AppHandle,
    request: &SftpTransferRequest,
    operation: &str,
    total_bytes: Option<u64>,
    cancel: Option<&AtomicBool>,
    transferred_bytes: &mut u64,
) -> Result<(), String> {
    const CHUNK_SIZE: usize = 1024 * 1024;
    let mut buffer = [0_u8; CHUNK_SIZE];

    loop {
        check_transfer_canceled(cancel)?;

        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => {
                return Err(format!("SFTP {} read failed: {}", operation, error));
            }
        };

        if let Err(error) = writer.write_all(&buffer[..read]) {
            return Err(format!("SFTP {} write failed: {}", operation, error));
        }

        *transferred_bytes += read as u64;
        emit_progress(
            app,
            request,
            operation,
            *transferred_bytes,
            total_bytes,
            false,
            None,
        );
    }

    writer
        .flush()
        .map_err(|error| format!("SFTP {} flush failed: {}", operation, error))
}

fn check_transfer_canceled(cancel: Option<&AtomicBool>) -> Result<(), String> {
    if cancel
        .map(|flag| flag.load(Ordering::SeqCst))
        .unwrap_or(false)
    {
        Err("Transfer canceled".to_string())
    } else {
        Ok(())
    }
}

fn finish_transfer(
    app: &AppHandle,
    request: &SftpTransferRequest,
    operation: &str,
    transferred_bytes: u64,
    total_bytes: Option<u64>,
    result: Result<(), String>,
) -> Result<(), String> {
    match result {
        Ok(()) => {
            emit_progress(
                app,
                request,
                operation,
                transferred_bytes,
                total_bytes,
                true,
                None,
            );
            Ok(())
        }
        Err(error) => {
            emit_progress(
                app,
                request,
                operation,
                transferred_bytes,
                total_bytes,
                true,
                Some(error.clone()),
            );
            Err(error)
        }
    }
}

fn local_entry_size(path: &Path) -> Result<u64, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Failed to inspect local path {}: {}", path.display(), error))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "Symbolic links are not supported for folder transfer: {}",
            path.display()
        ));
    }
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    if !metadata.is_dir() {
        return Err(format!("Unsupported local entry: {}", path.display()));
    }

    fs::read_dir(path)
        .map_err(|error| {
            format!(
                "Failed to read local directory {}: {}",
                path.display(),
                error
            )
        })?
        .try_fold(0_u64, |total, entry| {
            let entry = entry
                .map_err(|error| format!("Failed to read local directory entry: {}", error))?;
            local_entry_size(&entry.path()).map(|size| total.saturating_add(size))
        })
}

fn remote_entry_size(sftp: &Sftp, path: &Path, stat: &FileStat) -> Result<u64, String> {
    match file_type(stat) {
        "file" => Ok(stat.size.unwrap_or(0)),
        "directory" => remote_directory_entries(sftp, path)?.into_iter().try_fold(
            0_u64,
            |total, (child_path, child_stat)| {
                remote_entry_size(sftp, &child_path, &child_stat)
                    .map(|size| total.saturating_add(size))
            },
        ),
        "symlink" => Err(format!(
            "Symbolic links are not supported for folder transfer: {}",
            path.display()
        )),
        _ => Err(format!("Unsupported remote entry: {}", path.display())),
    }
}

fn remote_directory_entries(sftp: &Sftp, path: &Path) -> Result<Vec<(PathBuf, FileStat)>, String> {
    sftp.readdir(path)
        .map_err(|error| {
            format!(
                "SFTP directory listing failed for {}: {}",
                path.display(),
                error
            )
        })
        .map(|entries| {
            entries
                .into_iter()
                .filter(|(entry_path, _)| {
                    !matches!(
                        entry_path.file_name().and_then(|name| name.to_str()),
                        Some(".") | Some("..")
                    )
                })
                .collect()
        })
}

fn remote_child_path(parent: &Path, child: &std::ffi::OsStr) -> PathBuf {
    let base = parent.to_string_lossy().replace('\\', "/");
    let child = child.to_string_lossy();
    if base == "/" {
        PathBuf::from(format!("/{}", child))
    } else {
        PathBuf::from(format!("{}/{}", base.trim_end_matches('/'), child))
    }
}

fn ensure_remote_directory(sftp: &Sftp, path: &Path) -> Result<(), String> {
    if let Ok(stat) = sftp.lstat(path) {
        if file_type(&stat) == "directory" {
            return Ok(());
        }
        remove_remote_entry(sftp, path, &stat)?;
    }
    sftp.mkdir(path, 0o755)
        .map_err(|error| format!("SFTP mkdir failed for {}: {}", path.display(), error))
}

fn remove_remote_entry(sftp: &Sftp, path: &Path, stat: &FileStat) -> Result<(), String> {
    if file_type(stat) == "directory" {
        for (child_path, child_stat) in remote_directory_entries(sftp, path)? {
            remove_remote_entry(sftp, &child_path, &child_stat)?;
        }
        sftp.rmdir(path).map_err(|error| {
            format!(
                "SFTP directory delete failed for {}: {}",
                path.display(),
                error
            )
        })
    } else {
        sftp.unlink(path)
            .map_err(|error| format!("SFTP file delete failed for {}: {}", path.display(), error))
    }
}

fn ensure_local_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => return Ok(()),
        Ok(_) => remove_local_entry_if_exists(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "Failed to inspect local path {}: {}",
                path.display(),
                error
            ));
        }
    }
    fs::create_dir_all(path).map_err(|error| {
        format!(
            "Failed to create local directory {}: {}",
            path.display(),
            error
        )
    })
}

fn remove_local_entry_if_exists(path: &Path) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Failed to inspect local path {}: {}",
                path.display(),
                error
            ));
        }
    };

    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path).map_err(|error| {
            format!(
                "Failed to remove local directory {}: {}",
                path.display(),
                error
            )
        })
    } else {
        fs::remove_file(path)
            .map_err(|error| format!("Failed to remove local file {}: {}", path.display(), error))
    }
}

fn reserve_drag_out_paths(
    export_dir: &Path,
    remote_paths: Vec<String>,
) -> Result<Vec<SftpDragOutPath>, String> {
    let mut reserved_names = HashSet::new();
    remote_paths
        .into_iter()
        .map(|remote_path| {
            let trimmed_path = remote_path.trim_end_matches('/');
            let source_name = Path::new(trimmed_path)
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty() && *name != "." && *name != "..")
                .ok_or_else(|| format!("Remote path has no usable file name: {}", remote_path))?;
            let safe_name =
                unique_drag_out_name(&sanitize_drag_out_name(source_name), &mut reserved_names);
            let local_path = export_dir.join(safe_name);
            if !local_path.is_absolute() {
                return Err(format!(
                    "SFTP drag export path is not absolute: {}",
                    local_path.display()
                ));
            }
            Ok(SftpDragOutPath {
                remote_path,
                local_path: local_path.to_string_lossy().to_string(),
            })
        })
        .collect()
}

fn sanitize_drag_out_name(name: &str) -> String {
    let mut sanitized = name
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
            {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    sanitized = sanitized.trim_end_matches([' ', '.']).to_string();
    if sanitized.is_empty() {
        sanitized.push_str("remote-item");
    }

    let stem = sanitized
        .split_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(&sanitized)
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .and_then(|suffix| suffix.parse::<u8>().ok())
            .is_some_and(|number| (1..=9).contains(&number));
    if reserved {
        sanitized.insert(0, '_');
    }
    sanitized
}

fn unique_drag_out_name(name: &str, reserved_names: &mut HashSet<String>) -> String {
    if reserved_names.insert(name.to_lowercase()) {
        return name.to_string();
    }

    let path = Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(name);
    let extension = path.extension().and_then(|value| value.to_str());
    for copy_number in 2_u32.. {
        let candidate = match extension {
            Some(extension) if !extension.is_empty() => {
                format!("{} ({}).{}", stem, copy_number, extension)
            }
            _ => format!("{} ({})", stem, copy_number),
        };
        if reserved_names.insert(candidate.to_lowercase()) {
            return candidate;
        }
    }
    unreachable!("drag-out copy number range is unbounded")
}

fn cleanup_drag_out_cache(cache_root: &Path) {
    let Ok(entries) = fs::read_dir(cache_root) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age <= DRAG_OUT_CACHE_MAX_AGE {
            continue;
        }
        if file_type.is_dir() {
            let _ = fs::remove_dir_all(entry.path());
        } else {
            let _ = fs::remove_file(entry.path());
        }
    }
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

#[cfg(test)]
mod tests {
    use super::{local_entry_size, remote_child_path, reserve_drag_out_paths};
    use std::fs;
    use std::path::Path;

    #[test]
    fn local_entry_size_sums_nested_files() {
        let root = std::env::temp_dir().join(format!("gputerm-sftp-{}", uuid::Uuid::new_v4()));
        let nested = root.join("nested");
        fs::create_dir_all(&nested).expect("create test directory");
        fs::write(root.join("one.bin"), [1_u8, 2, 3]).expect("write first file");
        fs::write(nested.join("two.bin"), [4_u8, 5]).expect("write nested file");

        assert_eq!(local_entry_size(&root).expect("measure directory"), 5);

        fs::remove_dir_all(root).expect("remove test directory");
    }

    #[test]
    fn remote_child_path_uses_remote_separators() {
        assert_eq!(
            remote_child_path(Path::new("/srv/base"), std::ffi::OsStr::new("child.txt")),
            Path::new("/srv/base/child.txt")
        );
        assert_eq!(
            remote_child_path(Path::new("/"), std::ffi::OsStr::new("child.txt")),
            Path::new("/child.txt")
        );
    }

    #[test]
    fn drag_out_paths_are_absolute_safe_and_unique() {
        let root =
            std::env::temp_dir().join(format!("gputerm-sftp-drag-paths-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create drag path test directory");

        let paths = reserve_drag_out_paths(
            &root,
            vec![
                "/srv/report?.txt".to_string(),
                "/archive/report*.txt".to_string(),
                "/srv/CON".to_string(),
            ],
        )
        .expect("reserve drag-out paths");

        assert!(paths
            .iter()
            .all(|path| Path::new(&path.local_path).is_absolute()));
        assert!(paths[0].local_path.ends_with("report_.txt"));
        assert!(paths[1].local_path.ends_with("report_ (2).txt"));
        assert!(paths[2].local_path.ends_with("_CON"));

        fs::remove_dir_all(root).expect("remove drag path test directory");
    }
}
