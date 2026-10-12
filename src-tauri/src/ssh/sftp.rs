use crate::ssh::session::{
    open_ssh_session, target_for_active_session, with_ops_session, AppState, SshTarget,
};
use serde::{Deserialize, Serialize};
use ssh2::{FileStat, RenameFlags, Sftp};
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Emitter, State};

const OPS_TIMEOUT_MS: u32 = 10_000;
const DOWNLOAD_PART_SUFFIX: &str = ".gputerm-part";
const DRAG_OUT_CACHE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// Read/write unit for transfers.
const CHUNK_SIZE: usize = 256 * 1024;
/// Floor on how often an in-flight transfer reports progress to the webview.
const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(100);
/// Error text used for a user-requested cancellation.
const TRANSFER_CANCELED: &str = "Transfer canceled";

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
    /// Folder transfers: leave files that already exist at the destination
    /// instead of replacing them.
    #[serde(default)]
    keep_existing: bool,
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
    /// Set when the user cancelled. The frontend must not have to match on
    /// `error` text to tell a cancellation from a real failure.
    canceled: bool,
    /// Folder transfers only: the file being copied, how many are done, and
    /// how many entries were left alone (kept existing files, links).
    #[serde(skip_serializing_if = "Option::is_none")]
    current_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    files_done: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    files_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skipped: Option<u64>,
}

#[derive(Debug, Default)]
struct FolderProgress {
    current_file: Option<String>,
    files_done: u64,
    files_total: u64,
    skipped: u64,
}

/// What a transfer did. A single file reports `files: 1`.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SftpTransferSummary {
    files: u64,
    bytes: u64,
    /// Relative paths left alone because they already existed.
    skipped_existing: Vec<String>,
    /// Relative paths of symlinks and special files, which are never followed.
    skipped_other: Vec<String>,
}

impl SftpTransferSummary {
    fn single(bytes: u64) -> Self {
        Self {
            files: 1,
            bytes,
            ..Default::default()
        }
    }
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
#[tauri::command(async)]
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
) -> Result<SftpTransferSummary, String> {
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
) -> Result<SftpTransferSummary, String> {
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

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpRenameRequest {
    session_id: String,
    source_path: String,
    target_path: String,
}

/// Renames or moves a remote entry. Refuses to replace an existing target, so
/// a rename can never silently destroy another file or folder.
#[tauri::command]
pub async fn sftp_rename(
    state: State<'_, AppState>,
    request: SftpRenameRequest,
) -> Result<(), String> {
    let target = target_for_active_session(&state, &request.session_id)?;
    let ops = Arc::clone(&state.ops_sessions);
    tauri::async_runtime::spawn_blocking(move || {
        with_ops_session(&ops, &target, OPS_TIMEOUT_MS, |session| {
            let sftp = session
                .sftp()
                .map_err(|error| format!("SFTP failed to start: {}", error))?;
            let source = Path::new(&request.source_path);
            let destination = Path::new(&request.target_path);
            if source == destination {
                return Ok(());
            }
            if sftp.lstat(destination).is_ok() {
                return Err(format!("{} already exists", request.target_path));
            }
            sftp.rename(source, destination, None).map_err(|error| {
                format!(
                    "SFTP rename failed for {} to {}: {}",
                    request.source_path, request.target_path, error
                )
            })
        })
    })
    .await
    .map_err(|error| format!("SFTP rename task failed: {}", error))?
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
) -> Result<SftpTransferSummary, String> {
    let connection = open_ssh_session(&target)?;
    let sftp = connection
        .session
        .sftp()
        .map_err(|error| format!("SFTP failed to start: {}", error))?;
    let remote_path = Path::new(&request.remote_path);
    let remote_stat = sftp
        .lstat(remote_path)
        .map_err(|error| format!("SFTP stat failed for {}: {}", request.remote_path, error))?;
    match file_type(&remote_stat) {
        "directory" => return transfer_dir(&app, &sftp, &request, cancel, false),
        "file" => {}
        "symlink" => {
            return Err(format!(
                "Symbolic links are not supported for transfer: {}",
                remote_path.display()
            ))
        }
        _ => {
            return Err(format!(
                "Unsupported remote entry: {}",
                remote_path.display()
            ))
        }
    }

    let total_bytes = remote_stat.size.unwrap_or(0);
    let mut transferred_bytes = 0;
    emit_progress(&app, &request, "download", transferred_bytes, Some(total_bytes), false, None, None);
    let result = download_remote_file(
        &sftp,
        remote_path,
        Path::new(&request.local_path),
        &app,
        &request,
        Some(total_bytes),
        cancel,
        &mut transferred_bytes,
        None,
    );
    finish_transfer(&app, &request, "download", transferred_bytes, Some(total_bytes), result, None)?;
    Ok(SftpTransferSummary::single(transferred_bytes))
}

fn upload_path(
    app: AppHandle,
    target: SshTarget,
    request: SftpTransferRequest,
    cancel: Option<&AtomicBool>,
) -> Result<SftpTransferSummary, String> {
    let local_path = Path::new(&request.local_path);
    let metadata = fs::symlink_metadata(local_path).map_err(|error| {
        format!(
            "Failed to inspect local path {}: {}",
            local_path.display(),
            error
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "Symbolic links are not supported for transfer: {}",
            local_path.display()
        ));
    }
    if !metadata.is_dir() && !metadata.is_file() {
        return Err(format!("Unsupported local entry: {}", local_path.display()));
    }

    let connection = open_ssh_session(&target)?;
    let sftp = connection
        .session
        .sftp()
        .map_err(|error| format!("SFTP failed to start: {}", error))?;
    if metadata.is_dir() {
        return transfer_dir(&app, &sftp, &request, cancel, true);
    }

    let total_bytes = metadata.len();
    let token = transfer_token(request.transfer_id.as_deref());
    let mut transferred_bytes = 0;
    emit_progress(&app, &request, "upload", transferred_bytes, Some(total_bytes), false, None, None);
    let result = upload_file(
        &sftp,
        local_path,
        Path::new(&request.remote_path),
        &token,
        &app,
        &request,
        Some(total_bytes),
        cancel,
        &mut transferred_bytes,
        None,
    );
    finish_transfer(&app, &request, "upload", transferred_bytes, Some(total_bytes), result, None)?;
    Ok(SftpTransferSummary::single(transferred_bytes))
}

/// Copies a whole folder over one connection: plans the tree first (bounded,
/// links never followed), creates folders parents-first, then copies files,
/// skipping existing ones when the user chose to keep them.
fn transfer_dir(
    app: &AppHandle,
    sftp: &Sftp,
    request: &SftpTransferRequest,
    cancel: Option<&AtomicBool>,
    upload: bool,
) -> Result<SftpTransferSummary, String> {
    let operation = if upload { "upload" } else { "download" };
    let mut folder = FolderProgress::default();
    let mut transferred_bytes = 0;
    let mut total_bytes = None;
    emit_progress(app, request, operation, 0, None, false, None, None);

    let outcome = (|| {
        let remote_root = Path::new(&request.remote_path);
        let local_root = Path::new(&request.local_path);
        let plan = if upload {
            plan_tree(|relative| {
                check_transfer_canceled(cancel)?;
                list_local_folder(&join_local(local_root, relative))
            })?
        } else {
            plan_tree(|relative| {
                check_transfer_canceled(cancel)?;
                list_remote_folder(sftp, &join_remote(remote_root, relative))
            })?
        };
        let mut summary = SftpTransferSummary {
            skipped_other: plan.skipped,
            ..Default::default()
        };

        if upload {
            ensure_remote_directory(sftp, remote_root)?;
            for dir in &plan.dirs {
                ensure_remote_directory(sftp, &join_remote(remote_root, dir))?;
            }
        } else {
            ensure_local_directory(local_root)?;
            for dir in &plan.dirs {
                ensure_local_directory(&join_local(local_root, dir))?;
            }
        }

        let mut files = Vec::new();
        for (relative, size) in plan.files {
            let exists = if upload {
                sftp.lstat(&join_remote(remote_root, &relative)).is_ok()
            } else {
                fs::symlink_metadata(join_local(local_root, &relative)).is_ok()
            };
            if exists && request.keep_existing {
                summary.skipped_existing.push(relative);
            } else {
                files.push((relative, size));
            }
        }

        total_bytes = Some(files.iter().map(|(_, size)| *size).sum::<u64>());
        folder.files_total = files.len() as u64;
        emit_progress(app, request, operation, 0, total_bytes, false, None, Some(&folder));

        let token = transfer_token(request.transfer_id.as_deref());
        for (index, (relative, _)) in files.iter().enumerate() {
            check_transfer_canceled(cancel)?;
            folder.files_done = index as u64;
            folder.current_file = Some(relative.clone());
            let remote = join_remote(remote_root, relative);
            let local = join_local(local_root, relative);
            if upload {
                upload_file(
                    sftp, &local, &remote, &token, app, request, total_bytes, cancel,
                    &mut transferred_bytes, Some(&folder),
                )?;
            } else {
                download_remote_file(
                    sftp, &remote, &local, app, request, total_bytes, cancel,
                    &mut transferred_bytes, Some(&folder),
                )?;
            }
            summary.files += 1;
        }
        folder.files_done = files.len() as u64;
        folder.current_file = None;
        summary.bytes = transferred_bytes;
        Ok(summary)
    })();

    let result = outcome.as_ref().map(|_| ()).map_err(Clone::clone);
    folder.skipped = outcome
        .as_ref()
        .map(|summary| (summary.skipped_existing.len() + summary.skipped_other.len()) as u64)
        .unwrap_or(0);
    finish_transfer(app, request, operation, transferred_bytes, total_bytes, result, Some(&folder))?;
    outcome
}

/// Uploads one local file through a remote part file.
#[allow(clippy::too_many_arguments)]
fn upload_file(
    sftp: &Sftp,
    local_path: &Path,
    remote_path: &Path,
    token: &str,
    app: &AppHandle,
    request: &SftpTransferRequest,
    total_bytes: Option<u64>,
    cancel: Option<&AtomicBool>,
    transferred_bytes: &mut u64,
    folder: Option<&FolderProgress>,
) -> Result<(), String> {
    check_transfer_canceled(cancel)?;
    // A directory occupying the destination name is never removed to make room
    // for a file: the overwrite prompt only told the user something exists, not
    // that a whole tree would be deleted.
    if let Ok(stat) = sftp.lstat(remote_path) {
        if file_type(&stat) == "directory" {
            return Err(format!(
                "Cannot upload {}: a directory of the same name already exists at {}. \
                 Rename or remove it first.",
                local_path.display(),
                remote_path.display()
            ));
        }
    }
    let mut local_file = fs::File::open(local_path).map_err(|error| {
        format!(
            "Failed to open local file {}: {}",
            local_path.display(),
            error
        )
    })?;
    // Write to a hidden part file first so a failed or canceled upload never
    // truncates an existing remote file the user chose to overwrite.
    let part_path = remote_part_path(remote_path, token);
    let mut remote_file = sftp.create(&part_path).map_err(|error| {
        format!(
            "SFTP upload create failed for {}: {}",
            remote_path.display(),
            error
        )
    })?;
    if let Err(error) = copy_transfer_stream(
        &mut local_file,
        &mut remote_file,
        app,
        request,
        "upload",
        total_bytes,
        cancel,
        transferred_bytes,
        folder,
    ) {
        drop(remote_file);
        let _ = sftp.unlink(&part_path);
        return Err(error);
    }
    drop(remote_file);
    finalize_remote_part(sftp, &part_path, remote_path)
}

/// Moves a finished part file onto `remote_path`.
///
/// libssh2 only transmits rename flags once the server negotiates SFTP
/// protocol 5, and OpenSSH's sftp-server speaks version 3. There OVERWRITE is
/// dropped and the rename fails whenever the destination already exists, so
/// remove it and retry. That trades atomicity for finishing an overwrite the
/// user already confirmed; when the retry also fails the uploaded data is left
/// in the part file instead of being discarded.
fn finalize_remote_part(sftp: &Sftp, part_path: &Path, remote_path: &Path) -> Result<(), String> {
    let rename_flags = RenameFlags::OVERWRITE | RenameFlags::ATOMIC | RenameFlags::NATIVE;
    let mut finalize = sftp.rename(part_path, remote_path, Some(rename_flags));
    let mut replaced_destination = false;
    if finalize.is_err() && sftp.unlink(remote_path).is_ok() {
        replaced_destination = true;
        finalize = sftp.rename(part_path, remote_path, Some(rename_flags));
    }
    match finalize {
        Ok(()) => Ok(()),
        Err(error) if replaced_destination => Err(format!(
            "SFTP upload finalize failed for {}: {}. The uploaded data is still at {}.",
            remote_path.display(),
            error,
            part_path.display()
        )),
        Err(error) => {
            let _ = sftp.unlink(part_path);
            Err(format!(
                "SFTP upload finalize failed for {}: {}",
                remote_path.display(),
                error
            ))
        }
    }
}

/// `.{name}.{token}.gputerm-part` next to `remote_path`, joined with `/` so a
/// Windows build never inserts a backslash into a remote path.
fn remote_part_path(remote_path: &Path, token: &str) -> PathBuf {
    let name = remote_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "upload".to_string());
    let part_name = format!(".{}.{}{}", name, token, DOWNLOAD_PART_SUFFIX);
    match remote_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            remote_child_path(parent, std::ffi::OsStr::new(&part_name))
        }
        _ => PathBuf::from(part_name),
    }
}

/// A filesystem-safe token that keeps concurrent uploads of the same file
/// from sharing a part file.
fn transfer_token(transfer_id: Option<&str>) -> String {
    transfer_id
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 64
                && value
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
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
    folder: Option<&FolderProgress>,
) -> Result<(), String> {
    check_transfer_canceled(cancel)?;
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

    // Checked before any bytes move: a directory holding the destination name
    // must not be deleted to land a file there.
    if let Ok(metadata) = fs::symlink_metadata(local_path) {
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            return Err(format!(
                "Cannot download {}: a directory of the same name already exists at {}. \
                 Rename or remove it first.",
                remote_path.display(),
                local_path.display()
            ));
        }
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
        folder,
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
    folder: Option<&FolderProgress>,
) -> Result<(), String> {
    // Heap-allocated: a stack buffer this size is close to the default thread
    // stack on some platforms, and this runs under recursive directory walks.
    let mut buffer = vec![0_u8; CHUNK_SIZE];
    let mut last_emit = Instant::now();

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
        // Rate-limited so a multi-gigabyte transfer does not push thousands of
        // IPC events through the webview. The terminal `done` event is emitted
        // unconditionally by `finish_transfer`, so no completion is lost.
        if last_emit.elapsed() >= PROGRESS_EMIT_INTERVAL {
            last_emit = Instant::now();
            emit_progress(
                app,
                request,
                operation,
                *transferred_bytes,
                total_bytes,
                false,
                None,
                folder,
            );
        }
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
        Err(TRANSFER_CANCELED.to_string())
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
    folder: Option<&FolderProgress>,
) -> Result<(), String> {
    let error = result.as_ref().err().cloned();
    emit_progress(
        app,
        request,
        operation,
        transferred_bytes,
        total_bytes,
        true,
        error,
        folder,
    );
    result
}

/// What a folder holds, as paths relative to it with `/` separators.
#[derive(Debug, Default, PartialEq, Eq)]
struct TreePlan {
    /// Parents before children.
    dirs: Vec<String>,
    files: Vec<(String, u64)>,
    skipped: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Dir,
    File,
    /// Symlinks and special files: never followed, so a link cannot pull in
    /// a tree outside the folder or loop forever.
    Other,
}

/// One folder's entries: name, kind, size in bytes.
type Listing = Vec<(String, EntryKind, u64)>;

const MAX_TREE_ENTRIES: usize = 100_000;
const MAX_TREE_DEPTH: usize = 64;

/// Walks a folder breadth-first. `list` returns `(name, kind, size)` for the
/// folder at a relative path ("" is the root).
fn plan_tree(mut list: impl FnMut(&str) -> Result<Listing, String>) -> Result<TreePlan, String> {
    let mut plan = TreePlan::default();
    let mut queue = std::collections::VecDeque::from([(String::new(), 0_usize)]);
    let mut seen = 0_usize;
    while let Some((relative, depth)) = queue.pop_front() {
        let mut entries = list(&relative)?;
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        for (name, kind, size) in entries {
            if name == "." || name == ".." {
                continue;
            }
            seen += 1;
            if seen > MAX_TREE_ENTRIES {
                return Err(format!(
                    "The folder has more than {} entries; transfer smaller folders",
                    MAX_TREE_ENTRIES
                ));
            }
            let path = if relative.is_empty() {
                name
            } else {
                format!("{}/{}", relative, name)
            };
            match kind {
                EntryKind::Dir if depth + 1 >= MAX_TREE_DEPTH => plan.skipped.push(path),
                EntryKind::Dir => {
                    plan.dirs.push(path.clone());
                    queue.push_back((path, depth + 1));
                }
                EntryKind::File => plan.files.push((path, size)),
                EntryKind::Other => plan.skipped.push(path),
            }
        }
    }
    Ok(plan)
}

fn list_local_folder(path: &Path) -> Result<Listing, String> {
    let entries = fs::read_dir(path).map_err(|error| {
        format!(
            "Failed to read local directory {}: {}",
            path.display(),
            error
        )
    })?;
    let mut listed = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("Failed to read local directory entry: {}", error))?;
        // symlink_metadata: a link is reported as a link, not followed.
        let metadata = entry.path().symlink_metadata().map_err(|error| {
            format!("Failed to inspect {}: {}", entry.path().display(), error)
        })?;
        let kind = if metadata.is_dir() {
            EntryKind::Dir
        } else if metadata.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        listed.push((entry.file_name().to_string_lossy().into_owned(), kind, metadata.len()));
    }
    Ok(listed)
}

fn list_remote_folder(sftp: &Sftp, path: &Path) -> Result<Listing, String> {
    Ok(remote_directory_entries(sftp, path)?
        .into_iter()
        .filter_map(|(path, stat)| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            // READDIR attributes describe the entry itself, so a symlink is
            // reported as one rather than as its target.
            let kind = match file_type(&stat) {
                "directory" => EntryKind::Dir,
                "file" => EntryKind::File,
                _ => EntryKind::Other,
            };
            Some((name, kind, stat.size.unwrap_or(0)))
        })
        .collect())
}

/// Remote paths are POSIX whatever the local platform.
fn join_remote(root: &Path, relative: &str) -> PathBuf {
    if relative.is_empty() {
        return root.to_path_buf();
    }
    let root = root.to_string_lossy().replace('\\', "/");
    if root == "/" {
        return PathBuf::from(format!("/{}", relative));
    }
    PathBuf::from(format!("{}/{}", root.trim_end_matches('/'), relative))
}

fn join_local(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .filter(|part| !part.is_empty())
        .fold(root.to_path_buf(), |path, part| path.join(part))
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
        // Transfers never change an entry's type on the user's behalf.
        return Err(format!(
            "Cannot create remote directory {}: a file of the same name already exists. \
             Rename or remove it first.",
            path.display()
        ));
    }
    sftp.mkdir(path, 0o755)
        .map_err(|error| format!("SFTP mkdir failed for {}: {}", path.display(), error))
}

fn ensure_local_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => return Ok(()),
        Ok(_) => {
            return Err(format!(
                "Cannot create local directory {}: a file of the same name already exists. \
                 Rename or remove it first.",
                path.display()
            ))
        }
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

/// Clears a single file so a transfer can take its place. Directories are
/// refused rather than removed: no transfer may delete a tree to free up a name,
/// and the callers here only ever target a file or a `.gputerm-part` scratch
/// path.
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
        return Err(format!(
            "Refusing to replace the directory {} with a file. Rename or remove it first.",
            path.display()
        ));
    }
    fs::remove_file(path)
        .map_err(|error| format!("Failed to remove local file {}: {}", path.display(), error))
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

#[allow(clippy::too_many_arguments)]
fn emit_progress(
    app: &AppHandle,
    request: &SftpTransferRequest,
    operation: &str,
    transferred_bytes: u64,
    total_bytes: Option<u64>,
    done: bool,
    error: Option<String>,
    folder: Option<&FolderProgress>,
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
            canceled: error.as_deref() == Some(TRANSFER_CANCELED),
            error,
            current_file: folder.and_then(|folder| folder.current_file.clone()),
            files_done: folder.map(|folder| folder.files_done),
            files_total: folder.map(|folder| folder.files_total),
            skipped: folder.map(|folder| folder.skipped).filter(|count| *count > 0),
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
    use super::{
        ensure_local_directory, join_local, join_remote, list_local_folder, plan_tree,
        remove_local_entry_if_exists, remote_child_path, remote_part_path, reserve_drag_out_paths,
        transfer_token, EntryKind, Listing,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::fs;
    use std::path::Path;

    #[test]
    fn refuses_to_replace_a_local_directory_with_a_file() {
        let root = std::env::temp_dir().join(format!("gputerm-sftp-guard-{}", uuid::Uuid::new_v4()));
        let occupied = root.join("payload");
        fs::create_dir_all(occupied.join("nested")).expect("create occupied directory");
        fs::write(occupied.join("nested/keep.bin"), [7_u8]).expect("write nested file");

        let error = remove_local_entry_if_exists(&occupied).expect_err("must refuse a directory");
        assert!(error.contains("Refusing to replace the directory"), "{error}");
        // The tree the user already had is still there.
        assert!(occupied.join("nested/keep.bin").exists());

        fs::remove_dir_all(root).expect("remove test directory");
    }

    #[test]
    fn refuses_to_replace_a_local_file_with_a_directory() {
        let root = std::env::temp_dir().join(format!("gputerm-sftp-guard-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create test directory");
        let occupied = root.join("payload");
        fs::write(&occupied, [1_u8, 2, 3]).expect("write occupied file");

        let error = ensure_local_directory(&occupied).expect_err("must refuse a file");
        assert!(error.contains("a file of the same name already exists"), "{error}");
        assert_eq!(fs::read(&occupied).expect("file preserved").len(), 3);

        fs::remove_dir_all(root).expect("remove test directory");
    }

    #[test]
    fn clears_only_a_plain_file_in_the_way() {
        let root = std::env::temp_dir().join(format!("gputerm-sftp-guard-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create test directory");
        let stale = root.join("payload.gputerm-part");
        fs::write(&stale, [9_u8]).expect("write stale part file");

        remove_local_entry_if_exists(&stale).expect("stale part file is removable");
        assert!(!stale.exists());
        // A missing path is not an error.
        remove_local_entry_if_exists(&stale).expect("missing path is a no-op");

        fs::remove_dir_all(root).expect("remove test directory");
    }

    type FakeFolder<'a> = (&'a str, &'a [(&'a str, EntryKind, u64)]);

    fn fake_tree(tree: &[FakeFolder]) -> impl FnMut(&str) -> Result<Listing, String> {
        let map: HashMap<String, Listing> = tree
            .iter()
            .map(|(dir, entries)| {
                (
                    dir.to_string(),
                    entries
                        .iter()
                        .map(|(name, kind, size)| (name.to_string(), *kind, *size))
                        .collect(),
                )
            })
            .collect();
        move |relative: &str| {
            map.get(relative)
                .cloned()
                .ok_or_else(|| format!("no such folder {relative}"))
        }
    }

    #[test]
    fn plans_parents_before_children_and_skips_links() {
        let plan = plan_tree(fake_tree(&[
            (
                "",
                &[
                    ("b.txt", EntryKind::File, 3),
                    ("src", EntryKind::Dir, 0),
                    ("link", EntryKind::Other, 0),
                    (".", EntryKind::Dir, 0),
                ],
            ),
            (
                "src",
                &[("main.rs", EntryKind::File, 10), ("deep", EntryKind::Dir, 0)],
            ),
            ("src/deep", &[("x", EntryKind::File, 1)]),
        ]))
        .unwrap();
        assert_eq!(plan.dirs, vec!["src", "src/deep"]);
        assert_eq!(
            plan.files,
            vec![
                ("b.txt".to_string(), 3),
                ("src/main.rs".to_string(), 10),
                ("src/deep/x".to_string(), 1)
            ]
        );
        assert_eq!(plan.skipped, vec!["link"]);
    }

    #[test]
    fn a_listing_error_stops_the_plan() {
        let error = plan_tree(fake_tree(&[("", &[("gone", EntryKind::Dir, 0)])])).unwrap_err();
        assert!(error.contains("gone"));
    }

    #[test]
    fn joins_relative_paths_for_each_side() {
        assert_eq!(join_remote(Path::new("/home/u/"), "a/b"), PathBuf::from("/home/u/a/b"));
        assert_eq!(join_remote(Path::new("/home/u"), ""), PathBuf::from("/home/u"));
        assert_eq!(join_remote(Path::new("/"), "a"), PathBuf::from("/a"));
        assert_eq!(
            join_local(Path::new("/tmp/x"), "a/b"),
            Path::new("/tmp/x").join("a").join("b")
        );
    }

    #[test]
    fn walks_a_real_local_folder_without_following_links() {
        let root = std::env::temp_dir().join(format!("gputerm-tree-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.txt"), b"hello").unwrap();
        fs::write(root.join("sub/b.txt"), b"hi").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, root.join("sub/loop")).unwrap();

        let plan = plan_tree(|relative| list_local_folder(&join_local(&root, relative))).unwrap();
        assert_eq!(plan.dirs, vec!["sub"]);
        assert_eq!(
            plan.files,
            vec![("a.txt".to_string(), 5), ("sub/b.txt".to_string(), 2)]
        );
        #[cfg(unix)]
        assert_eq!(plan.skipped, vec!["sub/loop"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn remote_part_path_is_hidden_beside_the_target() {
        assert_eq!(
            remote_part_path(Path::new("/srv/data/model.bin"), "abc-1"),
            Path::new("/srv/data/.model.bin.abc-1.gputerm-part")
        );
        assert_eq!(
            remote_part_path(Path::new("/model.bin"), "t"),
            Path::new("/.model.bin.t.gputerm-part")
        );
        assert_eq!(transfer_token(Some("upload-42")), "upload-42");
        assert_ne!(transfer_token(Some("../evil")), "../evil");
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
