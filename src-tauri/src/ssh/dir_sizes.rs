//! Sizes of the folders directly inside a directory, for the disk popover's
//! "what is filling this mount" view.
//!
//! `du` can take minutes on a large tree, so it runs on demand only, one
//! level deep, on one filesystem (`-x`), at idle CPU and I/O priority, and is
//! killed on the server after `DU_LIMIT_SECS`. Whatever it finished by then is
//! still returned, marked incomplete. Remote scans use their own connection
//! so a slow `du` never holds up the shared operations connection that the
//! other popovers wait on.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::State;

use crate::ssh::resource_details::resolve_remote_os;
use crate::ssh::session::{open_ssh_session, target_for_active_session, AppState};
use crate::ssh::system_monitor::{
    local_os, run_local_command_with_timeout, run_remote_command_with_budget, shell_quote, RemoteOs,
};

/// How long `du` itself may run on the host.
const DU_LIMIT_SECS: u64 = 25;
/// The whole command, including login-shell startup and sorting.
const COMMAND_BUDGET_SECS: u64 = 35;
/// Folders returned; the rest are only counted.
const MAX_ENTRIES: usize = 200;
const MAX_PATH_LEN: usize = 4096;

/// Sessions with a scan in flight. One at a time per host keeps repeated
/// clicks from stacking up `du` processes on the server.
static RUNNING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryEntrySize {
    pub name: String,
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectorySizes {
    pub path: String,
    /// The whole directory; missing when `du` was stopped before finishing.
    pub total_bytes: Option<u64>,
    /// Files directly in the directory (total minus its folders), when known.
    pub files_bytes: Option<u64>,
    /// Largest first, at most `MAX_ENTRIES`.
    pub entries: Vec<DirectoryEntrySize>,
    /// Folders left out of `entries`.
    pub more_entries: usize,
    /// False when `du` hit the time limit; sizes are then lower bounds.
    pub complete: bool,
    /// Paths `du` could not read (permissions), so their sizes are missing.
    pub unreadable: u32,
}

/// `du` lines go through `sort` so only the largest reach the app; `N` counts
/// them all and `X` counts `du`'s error lines.
fn scan_script(path: &str) -> String {
    format!(
        r#"p={path}
if [ ! -d "$p" ]; then echo "E Not a directory: $p"; exit 0; fi
e=$(mktemp "${{TMPDIR:-/tmp}}/gputerm-du.XXXXXX") || {{ echo "E Could not create private scan error file"; exit 0; }}
trap 'rm -f "$e"' 0
set -- sh -c 'exec du -x -k -d 1 -- "$1" 2>"$2"' sh "$p" "$e"
if command -v stdbuf >/dev/null 2>&1; then set -- stdbuf -oL "$@"; fi
if command -v ionice >/dev/null 2>&1 && ionice -c3 true 2>/dev/null; then set -- ionice -c3 nice -n 19 "$@"; else set -- nice -n 19 "$@"; fi
if command -v timeout >/dev/null 2>&1; then set -- timeout -k 3 {limit} "$@"; elif command -v perl >/dev/null 2>&1; then set -- perl -e 'alarm shift; exec @ARGV; exit 127' {limit} "$@"; fi
"$@" | sort -rn | awk 'NR<={keep}{{print}} END{{print "N " NR}}'
echo "X $(wc -l < "$e" | tr -d ' ')"
rm -f "$e""#,
        path = shell_quote(path),
        limit = DU_LIMIT_SECS,
        keep = MAX_ENTRIES + 1,
    )
}

fn normalize_root_path(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/"
    } else {
        trimmed
    }
}

fn child_name(root: &str, child: &str) -> String {
    let root = normalize_root_path(root);
    let prefix = if root == "/" {
        "/".to_string()
    } else {
        format!("{root}/")
    };
    child.strip_prefix(&prefix).unwrap_or(child).to_string()
}

fn parse_sizes(root: &str, output: &str) -> Result<DirectorySizes, String> {
    let root = normalize_root_path(root);
    let mut total_kib = None;
    let mut entries = Vec::new();
    let mut counted = 0usize;
    let mut unreadable = 0u32;
    for line in output.lines() {
        if let Some(message) = line.strip_prefix("E ") {
            return Err(message.to_string());
        }
        if let Some(count) = line.strip_prefix("N ") {
            counted = count.trim().parse().unwrap_or(0);
            continue;
        }
        if let Some(count) = line.strip_prefix("X ") {
            unreadable = count.trim().parse().unwrap_or(0);
            continue;
        }
        let Some((kib, path)) = line.split_once('\t') else {
            continue;
        };
        let Ok(kib) = kib.trim().parse::<u64>() else {
            continue;
        };
        let normalized_path = normalize_root_path(path);
        if normalized_path == root {
            total_kib = Some(kib);
        } else {
            entries.push(DirectoryEntrySize {
                name: child_name(root, normalized_path),
                path: normalized_path.to_string(),
                bytes: kib.saturating_mul(1024),
            });
        }
    }
    entries.sort_by(|left, right| {
        right
            .bytes
            .cmp(&left.bytes)
            .then(left.name.cmp(&right.name))
    });
    entries.truncate(MAX_ENTRIES);
    let listed = entries.len() + usize::from(total_kib.is_some());
    let more_entries = counted.saturating_sub(listed);
    let total_bytes = total_kib.map(|kib| kib.saturating_mul(1024));
    let files_bytes = match total_bytes {
        Some(total) if more_entries == 0 => {
            Some(total.saturating_sub(entries.iter().map(|entry| entry.bytes).sum()))
        }
        _ => None,
    };
    Ok(DirectorySizes {
        path: root.to_string(),
        total_bytes,
        files_bytes,
        entries,
        more_entries,
        // `du` prints the directory itself last, so it is missing exactly
        // when the run was cut short.
        complete: total_kib.is_some(),
        unreadable,
    })
}

fn valid_path(path: &str) -> bool {
    path.starts_with('/') && path.len() <= MAX_PATH_LEN && !path.contains(['\0', '\n'])
}

fn validate_path_for_os(os: RemoteOs, path: &str, is_local: bool) -> Result<String, String> {
    if os == RemoteOs::Windows {
        return Err(if is_local {
            "Folder sizes are not available on Windows yet".to_string()
        } else {
            "Folder sizes are not available for Windows hosts yet".to_string()
        });
    }
    if !valid_path(path) {
        return Err("Choose an absolute folder path".to_string());
    }
    Ok(normalize_root_path(path).to_string())
}

/// Releases the per-session slot however the scan ends.
struct RunningGuard(String);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        if let Ok(mut running) = RUNNING.lock() {
            running.remove(&self.0);
        }
    }
}

#[tauri::command]
pub async fn directory_sizes(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
) -> Result<DirectorySizes, String> {
    let is_local = state
        .active_connections
        .lock()
        .map_err(|_| "Active session state is unavailable".to_string())?
        .get(&session_id)
        .map(|connection| connection.profile.is_local)
        .ok_or_else(|| "No active terminal session is available".to_string())?;
    let target = if is_local {
        None
    } else {
        Some(target_for_active_session(&state, &session_id)?)
    };
    {
        let mut running = RUNNING
            .lock()
            .map_err(|_| "Folder size state is unavailable".to_string())?;
        if !running.insert(session_id.clone()) {
            return Err(
                "A folder size scan is still running on this host. Try again when it finishes."
                    .to_string(),
            );
        }
    }
    let guard = RunningGuard(session_id.clone());
    let os_cache = Arc::clone(&state.remote_os_cache);

    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        let (normalized_path, output) = match target {
            None => {
                let os = local_os();
                let normalized_path = validate_path_for_os(os, &path, true)?;
                let script = scan_script(&normalized_path);
                let output = run_local_command_with_timeout(
                    os,
                    &script,
                    Duration::from_secs(COMMAND_BUDGET_SECS),
                )?;
                (normalized_path, output)
            }
            Some(target) => {
                let connection = open_ssh_session(&target)?;
                let os = resolve_remote_os(&os_cache, &target.session_id, &connection.session);
                let normalized_path = validate_path_for_os(os, &path, false)?;
                let script = scan_script(&normalized_path);
                let output = run_remote_command_with_budget(
                    &connection.session,
                    &script,
                    COMMAND_BUDGET_SECS,
                )?;
                (normalized_path, output)
            }
        };
        parse_sizes(&normalized_path, &output)
    })
    .await
    .map_err(|error| format!("Folder size task failed: {}", error))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_complete_scan() {
        let output = "\
104857600\t/data
62914560\t/data/datasets
31457280\t/data/checkpoints
10\t/data/.cache
N 4
X 0
";
        let sizes = parse_sizes("/data", output).unwrap();
        assert!(sizes.complete);
        assert_eq!(sizes.total_bytes, Some(104_857_600 * 1024));
        assert_eq!(
            sizes
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["datasets", "checkpoints", ".cache"]
        );
        assert_eq!(sizes.entries[0].path, "/data/datasets");
        assert_eq!(
            sizes.files_bytes,
            Some((104_857_600 - 62_914_560 - 31_457_280 - 10) * 1024)
        );
        assert_eq!(sizes.more_entries, 0);
    }

    #[test]
    fn a_cut_short_scan_is_partial() {
        let output = "62914560\t/data/datasets\nN 1\nX 3\n";
        let sizes = parse_sizes("/data", output).unwrap();
        assert!(!sizes.complete);
        assert_eq!(sizes.total_bytes, None);
        assert_eq!(sizes.files_bytes, None);
        assert_eq!(sizes.unreadable, 3);
        assert_eq!(sizes.entries.len(), 1);
    }

    #[test]
    fn counts_folders_beyond_the_limit() {
        let output = "900\t/\n500\t/usr\n300\t/var\nN 450\nX 12\n";
        let sizes = parse_sizes("/", output).unwrap();
        assert_eq!(sizes.entries[0].name, "usr");
        assert_eq!(sizes.more_entries, 447);
        assert_eq!(sizes.files_bytes, None);
    }

    #[test]
    fn reports_a_missing_directory_and_ignores_noise() {
        assert_eq!(
            parse_sizes("/nope", "motd line\nE Not a directory: /nope\n").unwrap_err(),
            "Not a directory: /nope"
        );
        let sizes = parse_sizes("/a", "Welcome!\n12\t/a/my dir\nbad\tline\n").unwrap();
        assert_eq!(sizes.entries[0].name, "my dir");
    }

    #[test]
    fn script_quotes_the_path_and_limits_du() {
        let script = scan_script("/data/it's here");
        assert!(script.starts_with("p='/data/it'\\''s here'"));
        assert!(script.contains("du -x -k -d 1 -- \"$1\" 2>\"$2\""));
        assert!(script.contains("sh \"$p\" \"$e\""));
        assert!(script.contains("stdbuf -oL"));
        assert!(script.contains("timeout -k 3 25"));
        assert!(script.contains("NR<=201"));
        assert!(script.contains("mktemp"));
        assert!(script.contains("trap 'rm -f \"$e\"' 0"));
        assert!(!script.contains("gputerm-du.$$"));
    }

    #[test]
    fn only_absolute_single_line_paths_are_scanned() {
        assert!(valid_path("/"));
        assert!(valid_path("/mnt/data disk"));
        assert!(!valid_path("data"));
        assert!(!valid_path("/a\nb"));
    }

    #[test]
    fn normalizes_trailing_slashes_for_root_matching() {
        assert_eq!(normalize_root_path("/"), "/");
        assert_eq!(normalize_root_path("///"), "/");
        assert_eq!(normalize_root_path("/data/"), "/data");
        assert_eq!(normalize_root_path("/data///"), "/data");

        let output = "100\t/data\n40\t/data/models\nN 2\nX 0\n";
        let sizes = parse_sizes("/data/", output).unwrap();
        assert!(sizes.complete);
        assert_eq!(sizes.path, "/data");
        assert_eq!(sizes.total_bytes, Some(100 * 1024));
        assert_eq!(sizes.entries.len(), 1);
        assert_eq!(sizes.entries[0].name, "models");

        let trailing_du_output = "100\t/data/\n40\t/data/models/\nN 2\nX 0\n";
        let sizes = parse_sizes("/data", trailing_du_output).unwrap();
        assert!(sizes.complete);
        assert_eq!(sizes.total_bytes, Some(100 * 1024));
        assert_eq!(sizes.entries[0].name, "models");
        assert_eq!(sizes.entries[0].path, "/data/models");
    }

    #[test]
    fn windows_os_check_precedes_posix_path_validation() {
        assert_eq!(
            validate_path_for_os(RemoteOs::Windows, "C:\\Users", false).unwrap_err(),
            "Folder sizes are not available for Windows hosts yet"
        );
        assert_eq!(
            validate_path_for_os(RemoteOs::Windows, "C:\\Users", true).unwrap_err(),
            "Folder sizes are not available on Windows yet"
        );
        assert_eq!(
            validate_path_for_os(RemoteOs::Linux, "C:\\Users", false).unwrap_err(),
            "Choose an absolute folder path"
        );
        assert_eq!(
            validate_path_for_os(RemoteOs::Linux, "/data/", false).unwrap(),
            "/data"
        );
    }

    #[cfg(unix)]
    #[test]
    fn collects_a_small_local_fixture_and_cleans_private_error_files() {
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture = Fixture(std::env::temp_dir().join(format!("gputerm-du-test-{}-{unique}", std::process::id())));
        let root = fixture.0.join("tree with spaces");
        let errors = fixture.0.join("errors");
        std::fs::create_dir_all(root.join("child folder")).unwrap();
        std::fs::create_dir_all(&errors).unwrap();
        std::fs::write(root.join("child folder/file"), b"controlled test data").unwrap();
        let output = std::process::Command::new("sh")
            .args(["-c", &scan_script(root.to_str().unwrap())])
            .env("TMPDIR", &errors)
            .output()
            .unwrap();
        assert!(output.status.success());
        let sizes = parse_sizes(root.to_str().unwrap(), &String::from_utf8_lossy(&output.stdout)).unwrap();
        assert!(sizes.complete);
        assert_eq!(sizes.unreadable, 0);
        assert_eq!(sizes.entries[0].name, "child folder");
        assert_eq!(std::fs::read_dir(errors).unwrap().count(), 0);
    }
}
