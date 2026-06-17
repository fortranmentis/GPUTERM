# GpuTerm

GpuTerm is a Tauri + React + TypeScript + Rust desktop MVP for managing SSH/SFTP sessions to GPU servers. It is shaped like an all-in-one SSH client, with a remote telemetry status bar that polls CPU, memory, disk, and NVIDIA GPU health through SSH.

## Features

- Local SSH session profiles with host, port, username, and private key path.
- Passwords are accepted for connection attempts but are not written to local JSON.
- xterm.js terminal connected to a Rust `ssh2` PTY shell.
- Terminal resize propagation from xterm to the remote PTY.
- SFTP directory browsing, upload, download, delete, and mkdir commands.
- SFTP drag-and-drop upload/download with transfer queue progress.
- SFTP transfer progress event payloads for large file workflows.
- Remote telemetry monitor that polls Linux CPU, memory, disk, and NVIDIA GPU metrics on a separate SSH session.
- Configurable telemetry interval, display mode, and ignored disk filesystem types.
- Trust-on-first-use `known_hosts.json` structure with host key mismatch detection.

## Project Structure

```text
src/
  components/
    RemoteTelemetryBar.tsx
    SessionSidebar.tsx
    SftpBrowser.tsx
    TerminalPane.tsx
  stores/
    sessionStore.ts
  types/
    gpu.ts
    session.ts
src-tauri/
  src/
    ssh/
      credentials.rs
      gpu_monitor.rs
      mod.rs
      session.rs
      sftp.rs
      system_monitor.rs
      terminal.rs
    lib.rs
    main.rs
```

## Installation

Prerequisites:

- Node.js 20 or newer
- npm 10 or newer
- Rust stable toolchain with `cargo` and `rustc`
- Tauri desktop prerequisites for your OS

Windows prerequisites:

- Microsoft Visual Studio Build Tools 2022
- Desktop development with C++ workload
- WebView2 Runtime
- Git for Windows

macOS prerequisites:

- Xcode Command Line Tools

```bash
xcode-select --install
```

Linux prerequisites vary by distribution. For Debian/Ubuntu, install the WebKitGTK, build, and SSL packages required by Tauri:

```bash
sudo apt update
sudo apt install -y libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

Clone the repository:

```bash
git clone https://github.com/fortranmentis/GPUTERM.git
cd GPUTERM
```

Install JavaScript dependencies:

```bash
npm install
```

Check that the Rust toolchain is available:

```bash
cargo --version
rustc --version
```

## Development Run

Start the full Tauri desktop app:

```bash
npm run tauri:dev
```

This starts the Vite frontend and opens the native Tauri desktop window. Use this mode for SSH terminal, SFTP, local folder browsing, and telemetry testing because those features depend on Tauri commands and events.

Run only the browser-based Vite frontend:

```bash
npm run dev
```

The Vite-only mode is useful for layout work, but native Tauri APIs such as the folder picker and Rust SSH commands are not available in a normal browser tab.

Run the test suite:

```bash
npm run test
cargo test --manifest-path src-tauri/Cargo.toml
```

Run production checks:

```bash
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

## Build

Create a packaged desktop build:

```bash
npm run tauri:build
```

The packaged app is emitted by Tauri under:

```text
src-tauri/target/release/bundle
```

Typical outputs are `.msi`/`.exe` on Windows, `.dmg`/`.app` on macOS, and `.deb`/`.AppImage` depending on the Linux bundle configuration.

## Usage

Start the app with:

```bash
npm run tauri:dev
```

Create an SSH session:

1. Open the session form in the left sidebar.
2. Enter `host`, `port`, `username`, and either a password or private key path.
3. Save the session.
4. Click the saved session to connect.

Session notes:

- Passwords are used only for the active connection and are not saved.
- Private key file contents are not stored; only the path is saved.
- Host key fingerprints are stored after the first successful trust-on-first-use connection.

Use the SSH terminal:

1. Connect to a saved session.
2. Type into the terminal pane.
3. Resize the window as needed; the remote PTY size is updated automatically.

Use the SFTP browser:

1. Connect to a session.
2. Use the remote path controls to list and navigate server directories.
3. Use `Browse...` beside `Local path` to choose a local directory with the OS folder picker.
4. Select a remote file and download it into the selected local directory.
5. Select a local file and upload it into the current remote directory.
6. Drag local files onto the remote panel to upload them into the current remote directory.
7. Drag remote files onto the local panel to download them into the selected local directory.
8. Use delete and new-folder actions from the SFTP panel as needed.

The last selected local directory is restored on the next app launch.

Transfer notes:

- Multiple files can be dropped at once.
- File transfers are streamed in 1 MiB chunks instead of loading the whole file into memory.
- The transfer queue shows filename, direction, source path, target path, progress, status, and per-file errors.
- If a target file already exists, GpuTerm asks whether to overwrite it before starting that file transfer.
- Directory drag-and-drop is detected but not transferred in the MVP.

Use remote telemetry:

1. Connect to a Linux server through SSH.
2. The bottom bar starts polling CPU, RAM, disk, and GPU status.
3. Change the telemetry interval to 1, 2, 5, or 10 seconds.
4. Switch the display mode between GPU only, system only, and GPU + system.
5. Click the disk summary to open the full disk detail popover.

For NVIDIA GPU servers, GpuTerm runs `nvidia-smi` every telemetry interval. If `nvidia-smi` is missing or the server has no NVIDIA GPU, the terminal remains connected and the GPU section shows an unavailable state.

## Troubleshooting

`npm install` fails:

- Confirm Node.js 20+ and npm 10+ are installed.
- Delete `node_modules` only when dependency installation is corrupted, then run `npm install` again.

`cargo` or `rustc` is not found:

- Install Rust with rustup.
- Restart the terminal so the Cargo bin directory is added to `PATH`.
- On Windows, the path is usually `%USERPROFILE%\.cargo\bin`.

`npm run tauri:dev` fails on Windows:

- Install Visual Studio Build Tools 2022 with the Desktop development with C++ workload.
- Make sure WebView2 Runtime is installed.
- Restart the terminal after installing build tools.

SSH authentication fails:

- Check host, port, username, password, and private key path.
- Ensure the remote server allows password or public key authentication.
- If the host key changed, remove the stale entry from `known_hosts.json` only after verifying the server fingerprint.

SFTP local browsing fails:

- Choose a directory that exists and is readable by the current OS user.
- On Windows, paths such as `C:\Users\user\Downloads` are supported.
- On macOS/Linux, paths such as `/Users/user/Downloads` or `/home/user/Downloads` are supported.

GPU metrics are unavailable:

- Confirm the remote server has NVIDIA drivers installed.
- Run `nvidia-smi` manually on the remote host.
- Non-NVIDIA servers can still show CPU, memory, and disk telemetry.

## Architecture Notes

The frontend calls Tauri commands through `@tauri-apps/api/core` and receives streaming updates through Tauri events.

- `connect_terminal` opens an SSH connection, creates a PTY, starts a shell, and emits `terminal-output`.
- `terminal_write` sends xterm input to the SSH channel.
- `terminal_resize` updates remote PTY dimensions.
- `system_monitor::start` opens a separate SSH connection so telemetry polling cannot break or block the terminal.
- Telemetry emits `remote-telemetry`, which contains CPU, memory, disk, GPU, and per-section errors in one payload.
- SFTP commands open separate SSH/SFTP sessions using the active in-memory credentials and emit `sftp-progress` during transfers.
- The SFTP local panel uses Tauri's native folder picker for the local path and stores the last selected directory in app settings.

Session profiles are stored in the user config directory:

- Windows: `%APPDATA%/GpuTerm/sessions.json`
- macOS: `~/Library/Application Support/GpuTerm/sessions.json`
- Linux: `$XDG_CONFIG_HOME/GpuTerm/sessions.json` or `~/.config/GpuTerm/sessions.json`

Host key fingerprints are stored in `known_hosts.json` in the same directory. The MVP uses trust-on-first-use: the first fingerprint is saved, and later mismatches are blocked with a clear error.

The most recent SFTP local directory is stored as `recentLocalPath` in `app_settings.json` in the same config directory. Passwords and private key contents are never written to this settings file.

## SFTP Local Browser

The SFTP panel has a `Browse...` button next to the local path field. It opens the operating system folder selection dialog through the Tauri dialog plugin. Choosing a folder updates the local path, validates that the directory exists and is accessible, reloads the local file list, and persists it as the default path for the next app launch. Cancelling the dialog leaves the current path unchanged.

Downloads are saved into the selected local directory. Uploads use the selected local file from the local file list and send it to the current remote directory. Paths are passed through platform-native strings so Windows, macOS, and Linux separators are preserved.

The same upload/download paths are used by drag-and-drop. Dropping local files on the remote SFTP panel uploads them to the current remote directory. Dropping remote files on the local panel downloads them to the selected local directory. Each dropped file becomes a transfer queue item and reports progress independently.

## Remote Telemetry

GpuTerm polls telemetry every 2 seconds by default. The UI can switch the interval to 1, 2, 5, or 10 seconds and can show GPU only, system only, or GPU + system.

CPU collection uses:

```bash
cat /proc/stat
cat /proc/loadavg
cat /proc/cpuinfo
nproc --all
nproc
lscpu
```

CPU usage is calculated from the previous and current aggregate `/proc/stat` samples. The first sample can show `n/a` until a second sample is available.

Memory collection uses:

```bash
cat /proc/meminfo
```

Memory is tracked internally in MiB and displayed in GiB. Used memory is calculated as `MemTotal - MemAvailable`.

Disk collection uses:

```bash
df -P -T -B1
```

The default hidden filesystem types are `tmpfs`, `devtmpfs`, `squashfs`, `proc`, `sysfs`, `cgroup`, `cgroup2`, and `overlay`. Mount points are prioritized as `/`, `/home`, `/data`, `/mnt*`, `/media*`, then everything else.

The bottom bar shows a compact disk summary with at most two mount points:

```text
Disk: / 46% · /data 43% · +2
```

Clicking the disk section opens the full disk detail popover with mount point, filesystem, filesystem type, used, available, total, and usage percentage. Disk sizes are formatted automatically as GiB or TiB. The usage column includes a progress bar; disks at 80% or higher are marked as warning, and disks at 90% or higher are marked as critical. The popover can temporarily show filesystems hidden by the default ignore list.

## GPU Monitoring Command

GpuTerm runs this command every 2 seconds after a successful SSH connection:

```bash
nvidia-smi --query-gpu=index,name,uuid,driver_version,power.draw,power.limit,temperature.gpu,utilization.gpu,utilization.memory,memory.total,memory.used,memory.free --format=csv,noheader,nounits
```

The Rust backend parses CSV rows into the frontend `GpuMetric` type and includes the result in `RemoteTelemetry.gpu`. If `nvidia-smi` is missing, returns no GPUs, times out, or exits non-zero, the GPU section displays `GPU metrics unavailable`. GPU polling errors are isolated from the terminal session and from CPU, memory, and disk collection.

## Security Notes

- Passwords are never saved to `sessions.json`.
- Passwords are held only in memory for active connections.
- Private key file contents are never read into app settings.
- `CredentialStore` is split into an interface and in-memory implementation so Windows Credential Manager, macOS Keychain, or Linux Secret Service can be added later.
- Host key mismatch is reported and blocks the connection.

## License

GpuTerm is licensed under the MIT License. See [LICENSE](./LICENSE) for details.

This project uses third-party open-source dependencies, including Tauri, React, xterm.js, ssh2, and related Rust/JavaScript packages. Their licenses remain with their respective authors.

## Known Limitations

- Only one active terminal session is fully wired in the MVP, though the command and state shape use session IDs for future tabs.
- Keyboard-interactive SSH authentication is not implemented yet.
- SFTP local browsing is directory based; recursive upload/download and directory drag-and-drop are not implemented yet.
- Transfer cancellation has a backend command placeholder, but active SFTP stream cancellation is not wired in the MVP.
- SFTP commands currently open fresh SSH sessions for reliability; pooled SFTP channels can be added later.
- The known_hosts MVP stores SHA-256 fingerprints in JSON, not OpenSSH known_hosts format.
- System telemetry is Linux-first and depends on `/proc`, `nproc`, `lscpu`, and GNU/POSIX-style `df`.
- GPU monitoring assumes NVIDIA GPUs and `nvidia-smi`; non-NVIDIA hosts still show CPU, memory, and disk telemetry.
