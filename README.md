# GpuTerm

GpuTerm is a Tauri + React + TypeScript + Rust desktop MVP for managing SSH/SFTP sessions to GPU servers. It is shaped like an all-in-one SSH client, with a remote telemetry status bar that polls CPU, memory, disk, and NVIDIA GPU health through SSH.

## Features

- Local SSH session profiles with host, port, username, and private key path.
- Passwords are accepted for connection attempts but are not written to local JSON.
- xterm.js terminal connected to a Rust `ssh2` PTY shell.
- Terminal resize propagation from xterm to the remote PTY.
- SFTP directory browsing, upload, download, delete, and mkdir commands.
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

## Development

Prerequisites:

- Node.js 20+
- Rust stable toolchain
- Tauri desktop prerequisites for your OS

Install dependencies:

```bash
npm install
```

Run the app in development:

```bash
npm run tauri:dev
```

Run only the Vite frontend:

```bash
npm run dev
```

## Build

```bash
npm run tauri:build
```

The packaged app is emitted by Tauri under `src-tauri/target/release/bundle`.

## Architecture Notes

The frontend calls Tauri commands through `@tauri-apps/api/core` and receives streaming updates through Tauri events.

- `connect_terminal` opens an SSH connection, creates a PTY, starts a shell, and emits `terminal-output`.
- `terminal_write` sends xterm input to the SSH channel.
- `terminal_resize` updates remote PTY dimensions.
- `system_monitor::start` opens a separate SSH connection so telemetry polling cannot break or block the terminal.
- Telemetry emits `remote-telemetry`, which contains CPU, memory, disk, GPU, and per-section errors in one payload.
- SFTP commands open separate SSH/SFTP sessions using the active in-memory credentials and emit `sftp-progress` during transfers.

Session profiles are stored in the user config directory:

- Windows: `%APPDATA%/GpuTerm/sessions.json`
- macOS: `~/Library/Application Support/GpuTerm/sessions.json`
- Linux: `$XDG_CONFIG_HOME/GpuTerm/sessions.json` or `~/.config/GpuTerm/sessions.json`

Host key fingerprints are stored in `known_hosts.json` in the same directory. The MVP uses trust-on-first-use: the first fingerprint is saved, and later mismatches are blocked with a clear error.

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

The default hidden filesystem types are `tmpfs`, `devtmpfs`, `squashfs`, `proc`, `sysfs`, `cgroup`, `cgroup2`, and `overlay`. Mount points under `/`, `/home`, `/data`, `/mnt`, and `/media` are prioritized in the bottom bar. Clicking the disk section shows the full non-ignored disk list.

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

## Known Limitations

- Only one active terminal session is fully wired in the MVP, though the command and state shape use session IDs for future tabs.
- Keyboard-interactive SSH authentication is not implemented yet.
- SFTP uses typed local paths instead of native file picker dialogs.
- SFTP commands currently open fresh SSH sessions for reliability; pooled SFTP channels can be added later.
- The known_hosts MVP stores SHA-256 fingerprints in JSON, not OpenSSH known_hosts format.
- System telemetry is Linux-first and depends on `/proc`, `nproc`, `lscpu`, and GNU/POSIX-style `df`.
- GPU monitoring assumes NVIDIA GPUs and `nvidia-smi`; non-NVIDIA hosts still show CPU, memory, and disk telemetry.
