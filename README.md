<div align="center">

# GpuTerm

**The all-in-one SSH/SFTP desktop client for GPU servers.**

Terminal, file transfers, and real-time CPU · RAM · Disk · NVIDIA GPU telemetry — in a single native window.

[![Release](https://img.shields.io/github/v/release/fortranmentis/GPUTERM?include_prereleases&label=release&color=2ea44f)](https://github.com/fortranmentis/GPUTERM/releases)
[![License: MIT](https://img.shields.io/github/license/fortranmentis/GPUTERM?color=blue)](./LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-8b5cf6)](#installation)
[![Built with Tauri](https://img.shields.io/badge/Tauri-2-FFC131?logo=tauri&logoColor=white)](https://tauri.app)
[![React](https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=white)](https://react.dev)
[![Rust](https://img.shields.io/badge/Rust-stable-DEA584?logo=rust&logoColor=white)](https://www.rust-lang.org)

[English](./README.md) · [한국어](./README.ko.md)

<img src="docs/screenshots/main.png" alt="GpuTerm main window: session sidebar with jump-host support, SSH terminal, SFTP browser, and the telemetry bar" width="850" />

</div>

---

Working on a remote GPU box usually means juggling an SSH client, an SFTP tool, and a second terminal running `watch nvidia-smi`. **GpuTerm replaces all three.** Connect once and get an xterm.js terminal, a drag-and-drop SFTP browser, and a live telemetry bar that polls CPU, memory, disk, logged-in users, and every NVIDIA GPU on the host — over its own SSH channel, so monitoring never blocks your shell.

> **Status:** beta. Download the latest prerelease from the [Releases](https://github.com/fortranmentis/GPUTERM/releases) page or build from source below.

## Features

### 🖥️ SSH Terminal
- Full PTY terminal powered by [xterm.js](https://xtermjs.org) and Rust [`ssh2`](https://crates.io/crates/ssh2)
- **Multiple concurrent sessions** — each keeps its own terminal, scrollback, and SFTP path; click a connected profile in the sidebar to switch
- **ProxyJump** — tunnel through a saved profile as a bastion (per-key-type host verification along the way)
- Password, private key (with passphrase), and SSH agent authentication
- UTF-8 safe streaming — multibyte characters (한글, 日本語, emoji) survive chunked reads
- MOTD and early output are buffered and replayed, never lost to connection races
- Automatic remote PTY resize and SSH keepalive

### 📁 SFTP Browser
- Side-by-side remote/local panels with drag-and-drop upload & download
- Streaming 1 MiB chunked transfers with a progress queue and **per-file cancellation**
- Downloads are written to a temporary file and atomically renamed — no partial files ever
- Overwrite confirmation, delete, mkdir, and a native OS folder picker
- Resizable split between terminal and SFTP panes (persisted across launches)

### 📊 Live Telemetry
- Bottom status bar polling CPU, RAM, disk, logged-in users, and **NVIDIA, AMD (ROCm), and Intel** GPUs every 1–10 s
- Click any section for a **draggable, resizable detail popover**: per-core CPU usage, top processes, VRAM/power/temperature per GPU, full mount list
- **Pop any detail view out into its own OS window** — it refreshes independently and closes with its session
- Telemetry runs on a dedicated SSH connection (per session) with automatic reconnect and exponential backoff
- Non-NVIDIA hosts gracefully fall back to system-only metrics

### 🔐 Security by default
- Passwords live in memory only — never written to disk
- Trust-on-first-use host key prompt with SHA-256 fingerprint; mismatches block the connection
- Restrictive Tauri Content Security Policy in production and development

## Installation

### Prebuilt binaries

Grab the installer for your OS from the [latest release](https://github.com/fortranmentis/GPUTERM/releases) (`.msi`/`.exe`, `.dmg`, `.deb`/`.AppImage`).

### Build from source

**Prerequisites:** [Node.js](https://nodejs.org) ≥ 20, npm ≥ 10, [Rust](https://rustup.rs) stable, and the [Tauri prerequisites](https://tauri.app/start/prerequisites/) for your OS.

<details>
<summary>Per-OS prerequisite details</summary>

**Windows**
- Visual Studio Build Tools 2022 with the *Desktop development with C++* workload
- WebView2 Runtime (preinstalled on Windows 10/11)
- Git for Windows
- [Strawberry Perl](https://strawberryperl.com) (`winget install StrawberryPerl.StrawberryPerl`) — required to compile the vendored OpenSSL that backs the SSH library

**macOS**
```bash
xcode-select --install
```

**Linux (Debian/Ubuntu)**
```bash
sudo apt update
sudo apt install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

</details>

```bash
git clone https://github.com/fortranmentis/GPUTERM.git
cd GPUTERM
npm install

# Run the desktop app in development mode
npm run tauri:dev

# Package a distributable build (output: src-tauri/target/release/bundle)
npm run tauri:build
```

> `npm run dev` starts the Vite frontend alone — useful for layout work, but SSH/SFTP/telemetry require the full Tauri app.

## Usage

1. **Create a profile** — enter host, port, username, and a password or private key path in the sidebar. Press **New** to start a fresh profile, **Save** to keep it.
2. **Connect** — on first contact GpuTerm shows the server's SHA-256 host key fingerprint and asks for confirmation before trusting it. Connect as many servers as you like; connected profiles show a green dot, and clicking one switches the whole view to that session.
3. **Work** — type in the terminal, drag files between the SFTP panels, and watch live metrics in the bottom bar. Click CPU / RAM / Disk / GPU / Users for detail popovers you can drag around, resize, or pop out into separate windows with the ↗ button.

<details>
<summary>SFTP transfer details</summary>

- Drop multiple files at once; each becomes an independent queue item with progress, status, and error reporting.
- Running transfers can be canceled individually from the queue.
- If the target file exists, GpuTerm asks before overwriting.
- The last local directory is remembered across launches.
- Directory drag-and-drop is detected but not yet transferred (see [Roadmap](#roadmap--known-limitations)).

</details>

<details>
<summary>Telemetry configuration</summary>

- **Interval:** 1, 2 (default), 5, or 10 seconds — detail popovers poll on the same cadence.
- **Mode:** GPU + System, GPU only, or System only.
- **Ignore FS:** comma-separated filesystem types hidden from the disk summary (default: `tmpfs`, `devtmpfs`, `squashfs`, `proc`, `sysfs`, `cgroup`, `cgroup2`, `overlay`). The disk popover can temporarily reveal them.
- Mount points are prioritized `/` → `/home` → `/data` → `/mnt*` → `/media*` → others; disks ≥ 80% are flagged warning, ≥ 90% critical.

</details>

<details>
<summary>Remote commands executed for telemetry</summary>

All metrics come from standard tools over SSH — nothing is installed on the server.

| Section | Commands |
| --- | --- |
| CPU | `/proc/stat`, `/proc/loadavg`, `/proc/cpuinfo`, `nproc`, `lscpu` |
| Memory | `/proc/meminfo` |
| Disk | `df -P -T -B1` |
| Users | `who` |
| GPU | `nvidia-smi` (NVIDIA), `rocm-smi --json` (AMD/ROCm), `xpu-smi` / `intel_gpu_top` (Intel) — auto-detected per host |
| Top processes | `ps -eo … --sort=-%cpu` / `--sort=-rss` |

Commands run with a 3-second timeout on a dedicated SSH connection. GpuTerm detects which GPU tools exist on each host and shows a vendor tag on every card; `intel_gpu_top` needs root or `CAP_PERFMON`. If no GPU tool is present, the GPU section reports unavailable while everything else keeps working.

</details>

## Architecture

```
┌───────────────────────────── Tauri window ─────────────────────────────┐
│  React 19 + TypeScript + Zustand + xterm.js                            │
│    invoke() ──────────────► Tauri commands (Rust)                      │
│    listen() ◄────────────── terminal-output · remote-telemetry ·       │
│                             sftp-progress · terminal-closed            │
├────────────────────────────────────────────────────────────────────────┤
│  Rust backend (ssh2 / libssh2)                                         │
│    • Terminal      – PTY shell, dedicated connection per session       │
│    • Telemetry     – own connection, auto-reconnect with backoff       │
│    • SFTP ops      – pooled per-session "operations" connection        │
│    • Bulk transfer – dedicated connection per file, cancellable        │
└────────────────────────────────────────────────────────────────────────┘
```

Long-running work is isolated: blocking SSH I/O runs on `spawn_blocking` threads so the UI never freezes, and each concern (shell / telemetry / transfers) fails independently.

**Data locations** (`%APPDATA%\GpuTerm` on Windows, `~/Library/Application Support/GpuTerm` on macOS, `~/.config/GpuTerm` on Linux):

| File | Contents |
| --- | --- |
| `sessions.json` | Session profiles — host, port, username, key *path* only |
| `known_hosts.json` | Approved SHA-256 host key fingerprints |
| `app_settings.json` | UI preferences such as the last local SFTP directory |

Passwords and private key contents are **never** written to any of these files.

## Development

```bash
npm run test                                   # frontend tests (Vitest)
cargo test --manifest-path src-tauri/Cargo.toml # backend tests
npm run build                                  # TypeScript + Vite production build
```

<details>
<summary>Project layout</summary>

```
src/                    React frontend
  components/           TerminalPane, SftpBrowser, RemoteTelemetryBar, popovers…
  stores/               Zustand stores (session, transfers)
  utils/                Shared formatters, terminal buffer, disk priority
src-tauri/src/ssh/      Rust backend
  terminal.rs           PTY shell + UTF-8 safe reader
  system_monitor.rs     Telemetry loop + parsers
  resource_details.rs   On-demand CPU/RAM/GPU detail collection
  sftp.rs               Transfers, cancellation, SFTP commands
  session.rs            Connections, host keys, profiles, connection pool
```

</details>

## Troubleshooting

| Symptom | Check |
| --- | --- |
| `tauri:dev` fails on Windows | VS Build Tools 2022 (C++ workload) + WebView2 Runtime installed, then restart the terminal |
| `cargo` not found | Install via [rustup](https://rustup.rs), reopen the terminal (`%USERPROFILE%\.cargo\bin` on PATH) |
| SSH auth fails | Verify host/port/user/credentials; confirm the server allows the auth method |
| Host key mismatch | Verify the server fingerprint out-of-band, then remove the stale entry from `known_hosts.json` |
| GPU shows unavailable | Confirm a GPU tool is installed (`nvidia-smi`, `rocm-smi`, `xpu-smi`, or `intel_gpu_top`); other metrics still work regardless |

## Roadmap / Known limitations

- Keyboard-interactive SSH authentication is not implemented
- Recursive directory upload/download and transfer resume are not implemented
- `known_hosts.json` uses SHA-256 fingerprints, not the OpenSSH known_hosts format
- Telemetry is Linux-first (`/proc`, `lscpu`, POSIX `df`); GPU monitoring uses `nvidia-smi`, `rocm-smi`, `xpu-smi`, or `intel_gpu_top` (AMD support currently targets `rocm-smi`)

Issues and pull requests are welcome — please run the test suites above before submitting.

## License

[MIT](./LICENSE) © GpuTerm contributors. Built with [Tauri](https://tauri.app), [React](https://react.dev), [xterm.js](https://xtermjs.org), and [ssh2](https://crates.io/crates/ssh2); third-party licenses remain with their authors.
