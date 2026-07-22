<div align="center">

# GpuTerm

**The all-in-one SSH/SFTP desktop client for GPU servers.**

Terminal, file transfers, and real-time CPU · RAM · Disk · GPU telemetry (NVIDIA / AMD / Intel / Apple Silicon) — in a single native window.

[![Release](https://img.shields.io/github/v/release/fortranmentis/GPUTERM?include_prereleases&label=release&color=2ea44f)](https://github.com/fortranmentis/GPUTERM/releases)
[![Release Build](https://github.com/fortranmentis/GPUTERM/actions/workflows/release.yml/badge.svg)](https://github.com/fortranmentis/GPUTERM/actions/workflows/release.yml)
[![Downloads](https://img.shields.io/github/downloads/fortranmentis/GPUTERM/total?color=8b5cf6)](https://github.com/fortranmentis/GPUTERM/releases)
[![License: PolyForm Noncommercial](https://img.shields.io/badge/license-PolyForm%20Noncommercial%201.0.0-blue)](./LICENSE)
[![Built with Tauri](https://img.shields.io/badge/Tauri-2-FFC131?logo=tauri&logoColor=white)](https://tauri.app)
[![React](https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=white)](https://react.dev)
[![Rust](https://img.shields.io/badge/Rust-stable-DEA584?logo=rust&logoColor=white)](https://www.rust-lang.org)

[English](./README.md) · [한국어](./README.ko.md)

<img src="docs/screenshots/main.png" alt="GpuTerm main window: session sidebar with jump-host support, SSH terminal, SFTP browser, and the telemetry bar" width="850" />

</div>

---

Working on a remote GPU box usually means juggling an SSH client, an SFTP tool, and a second terminal running `watch nvidia-smi`. **GpuTerm replaces all three.** Connect once and get an xterm.js terminal, a drag-and-drop SFTP browser, and a live telemetry bar that polls CPU, memory, disk, logged-in users, and every GPU on the host — NVIDIA, AMD, Intel, or Apple Silicon, on Linux, macOS, and Windows remotes alike — over its own SSH channel, so monitoring never blocks your shell.

Nothing is ever installed on your servers: every metric comes from one-shot standard commands (`nvidia-smi`, `/proc`, `sysctl`, PowerShell CIM, …) over SSH, and no admin/root rights are required for the core metrics.

> **Status:** beta. Prebuilt installers for Windows, macOS, and Linux are attached to every [release](https://github.com/fortranmentis/GPUTERM/releases); you can also build from source below.

## Table of contents

- [Features](#features)
- [What you can monitor](#what-you-can-monitor)
- [Installation](#installation)
- [Usage](#usage)
- [Architecture](#architecture)
- [Development](#development)
- [FAQ](#faq)
- [Troubleshooting](#troubleshooting)
- [Roadmap / Known limitations](#roadmap--known-limitations)
- [License](#license)

## Features

### 🖥️ SSH Terminal
- Full PTY terminal powered by [xterm.js](https://xtermjs.org) and Rust [`ssh2`](https://crates.io/crates/ssh2)
- **Multiple concurrent sessions** — each keeps its own terminal, scrollback, and SFTP path; click a connected profile in the sidebar to switch
- **Up to four flexible terminal cells** — place a new shell or another saved session to the left, right, top, or bottom of the focused pane; choose its initial ratio and drag dividers to resize nested layouts
- **Collapsible host selector** — hide the sidebar for more workspace and reopen it from the top-left button; full profile fields appear only for **New**, while saved profiles show a connect-time credential prompt
- **ProxyJump** — tunnel through a saved profile as a bastion (per-key-type host verification along the way)
- **Native local terminal** — open the current machine without SSH and use the same terminal splits and monitoring UI
- Password, private key (with passphrase), and SSH agent authentication
- UTF-8 safe streaming — multibyte characters (한글, 日本語, emoji) survive chunked reads
- **CJK input works correctly** — Korean IME composition in the terminal is handled with the same backspace-rewrite protocol native terminals use, fixing the jamo-splitting bug in WebKit-based webviews
- MOTD and early output are buffered and replayed, never lost to connection races
- Serialized burst input plus a fully nonblocking SSH/TCP transport keeps simultaneous keys, modifier chords, and key repeat responsive without racing the terminal reader
- Automatic remote PTY resize and SSH keepalive, including ProxyJump tunnels

### 📁 SFTP Browser
- Side-by-side remote/local panels with drag-and-drop upload & download
- **Paste uploads from desktop file managers** — copy local files in Nautilus or another URI-list-aware file manager, focus the remote pane, and paste
- Streaming 1 MiB chunked transfers with a progress queue and **per-file cancellation**
- Downloads are written to a temporary file and atomically renamed — no partial files ever
- Overwrite confirmation, delete, mkdir, and a native OS folder picker
- Resizable split between terminal and SFTP panes (persisted across launches)
- **Collapsible SFTP panel** — close it with the directional panel button and restore it from the top-right; the terminal immediately expands into the freed width

### 📊 Live Telemetry
- Bottom status bar polling CPU, RAM, disk, logged-in users, and GPUs every 1–10 s — on **local or remote Linux, macOS, and Windows hosts**
- **Collapsible monitoring bar** — close it independently and restore it from the bottom-right; visibility is remembered across launches
- **NVIDIA, AMD, Intel, and Apple Silicon** GPUs are auto-detected per host; every card carries a vendor tag
- **Hybrid iGPU + dGPU hosts show both cards** — on Windows, counters are attributed to adapters by their DirectX LUID, so the integrated GPU is never mistaken for the discrete one
- Click any section for a **draggable, resizable detail popover** whose tables expand with the window: per-core CPU usage, top processes, VRAM/power/temperature per GPU, full mount list
- **Pop any detail view out into its own OS window** — it refreshes independently and closes with its session
- Remote telemetry runs on a dedicated SSH connection with automatic reconnect; local telemetry executes collectors directly without SSH
- Hosts without any GPU gracefully fall back to system-only metrics

### 🔐 Security by default
- Passwords and key passphrases are stored only in the local `credentials.enc` vault: Argon2id derives a 256-bit key from your GpuTerm master password, and AES-256-GCM encrypts and authenticates the complete credential payload
- The master password and derived key are kept in memory only for the current app run; secrets are never written in plaintext or included in `sessions.json`
- Saved-session password fields show a secure mask while keeping the actual secret out of the webview; leaving the field blank reuses the vault entry
- Trust-on-first-use host key prompt with SHA-256 fingerprint; mismatches block the connection
- Restrictive Tauri Content Security Policy in production and development

> **Upgrading to 1.1.2-beta:** GpuTerm no longer accesses macOS Keychain, Windows Credential Manager, or Linux Secret Service/libsecret. Existing OS-vault entries are left untouched but are not imported; create a GpuTerm master password and enter each SSH password again after upgrading.

## What you can monitor

| Metric | Linux | macOS (Apple Silicon) | Windows (OpenSSH Server) |
| --- | :-: | :-: | :-: |
| CPU model · cores · usage | ✅ | ✅ (P/E core split) | ✅ |
| Load average | ✅ | ✅ | — (doesn't exist on Windows) |
| Memory + swap | ✅ | ✅ (Activity Monitor semantics) | ✅ (page file as swap) |
| Disks / mounts | ✅ | ✅ | ✅ (fixed drives) |
| Logged-in users | ✅ `who` | ✅ `who` | ✅ `quser` (absent on Home editions) |
| NVIDIA GPU (util · VRAM · power · temp · processes) | ✅ `nvidia-smi` | — | ✅ `nvidia-smi` |
| AMD GPU | ✅ `rocm-smi` (full) | — | ◐ WDDM counters (util + VRAM) |
| Intel GPU | ◐ `xpu-smi` / `intel_gpu_top` | — | ◐ WDDM counters (util + VRAM) |
| Apple GPU | — | ◐ util + memory (power/temp need root) | — |
| Detail popovers (per-core CPU, top processes) | ✅ | ✅ (no per-core without root) | ✅ |

✅ full support ◐ partial (see [known limitations](#roadmap--known-limitations)) — the exact remote commands are listed under [Usage](#usage).

## Installation

### Prebuilt installers

Download from the [latest release](https://github.com/fortranmentis/GPUTERM/releases):

| OS | File | Notes |
| --- | --- | --- |
| Windows 10/11 (x64) | `GpuTerm_x.y.z_x64-setup.exe` | NSIS installer |
| macOS (Apple Silicon) | `GpuTerm_x.y.z_aarch64.dmg` | Intel Macs: build from source for now |
| Debian / Ubuntu (x64) | `GpuTerm_x.y.z_amd64.deb` | `sudo apt install ./GpuTerm_*.deb` |

<details>
<summary>“Unknown publisher” / Gatekeeper warnings</summary>

Beta builds are not signed with a trusted publisher/developer identity or notarized, so your OS may warn on first launch. The macOS app bundle is fully ad-hoc signed inside-out (nested Mach-O code first, then the final application bundle) and verified for integrity before the DMG is published:

- **Windows** — SmartScreen shows *“Windows protected your PC”*: click **More info → Run anyway**.
- **macOS** — if Gatekeeper blocks the ad-hoc-signed app, right-click **GpuTerm.app → Open → Open**, or run `xattr -cr /Applications/GpuTerm.app` once.

The installers are built on GitHub Actions from the tagged source (see [Releases & CI](#releases--ci)), so you can always audit exactly what went into them — or build your own below.

After copying the app to `/Applications`, you can verify the sealed bundle yourself:

```bash
codesign --verify --deep --strict --verbose=2 /Applications/GpuTerm.app
```

</details>

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

1. **Unlock the local vault** — choose a GpuTerm master password on first launch; on later launches enter it once to unlock saved credentials for that app run.
2. **Create a profile** — enter host, port, username, and a password or private key path in the sidebar. Press **New** to start a fresh profile, **Save** to keep it. To route through a bastion, pick any saved profile as the **Jump host**.
3. **Connect** — on first contact GpuTerm shows the server's SHA-256 host key fingerprint and asks for confirmation before trusting it. Connect as many servers as you like; connected profiles show a green dot, and clicking one switches the whole view to that session.
4. **Split terminals** — use the columns button to open another shell for the focused session, or the **+** button to add a different saved session. Choose left/right/top/bottom placement and the new pane's initial size before adding it.
5. **Work** — type in the terminal, drag or paste files into the remote SFTP panel, and watch live metrics in the bottom bar. Click CPU / RAM / Disk / GPU / Users for detail popovers you can drag around, resize, or pop out into separate windows with the ↗ button.

<details>
<summary>Terminal split controls</summary>

- The **columns** button opens another independent PTY shell for the currently focused session.
- The **+** button lists saved profiles. A live session is added immediately; a disconnected profile asks for its password/key passphrase (and jump-host password when applicable) before connecting.
- Up to four cells can be nested horizontally and vertically. Choose a 20–80% initial size, then drag any divider to adjust the ratio. Each mixed-session cell displays its profile name.
- Clicking a cell makes that session active for SFTP and telemetry while preserving the split layout. The cell's **×** button closes that terminal pane.

</details>

<details>
<summary>Workspace panel controls</summary>

- The host selector, SFTP browser, and monitoring bar each have a directional close button in their header.
- Reopen a hidden panel from its matching workspace edge: host selector at the top-left, SFTP at the top-right, and monitoring at the bottom-right.
- Each panel's open/closed state is saved locally. Hiding SFTP expands terminal width; hiding monitoring expands terminal height.

</details>

<details>
<summary>SFTP transfer details</summary>

- Drop multiple files at once; each becomes an independent queue item with progress, status, and error reporting.
- Running transfers can be canceled individually from the queue.
- If the target file exists, GpuTerm asks before overwriting.
- Files copied in Nautilus and compatible desktop file managers can be pasted into the focused remote pane to enqueue uploads.
- The last local directory is remembered across launches.
- Directory drag-and-drop is detected but not yet transferred (see [Roadmap](#roadmap--known-limitations)).

</details>

<details>
<summary>Telemetry configuration</summary>

- **Interval:** 1, 2 (default), 5, or 10 seconds — detail popovers poll on the same cadence.
- **Mode:** GPU + System, GPU only, or System only.
- **Ignore FS:** comma-separated filesystem types hidden from the disk summary (default: `tmpfs`, `devtmpfs`, `squashfs`, `proc`, `sysfs`, `cgroup`, `cgroup2`, `overlay`, `devfs`, `autofs`). The disk popover can temporarily reveal them.
- Mount points are prioritized `/` → `/home` → `/data` → `/mnt*` → `/media*` → drive letters → others; disks ≥ 80% are flagged warning, ≥ 90% critical.

</details>

<details>
<summary>Remote commands executed for telemetry</summary>

All metrics come from standard tools over SSH — nothing is installed on the server.

| Section | Linux | macOS | Windows |
| --- | --- | --- | --- |
| CPU | `/proc/stat`, `/proc/loadavg`, `/proc/cpuinfo`, `nproc`, `lscpu` | `sysctl` (brand, cores, P/E split, loadavg), `top -l 2` | `Win32_Processor`, `Win32_PerfRawData_PerfOS_Processor` (CIM) |
| Memory | `/proc/meminfo` | `sysctl hw.memsize`, `vm_stat`, `vm.swapusage` | `Win32_OperatingSystem`, `Win32_PageFileUsage` (CIM) |
| Disk | `df -P -T -B1` | `df -P -k` + `mount` | `Win32_LogicalDisk` (fixed drives) |
| Users | `who` | `who` | `quser` |
| GPU | `nvidia-smi` (NVIDIA), `rocm-smi --json` (AMD/ROCm), `xpu-smi` / `intel_gpu_top` (Intel) — auto-detected | `ioreg -c IOAccelerator` (Apple GPU utilization, no root needed) | `nvidia-smi` (NVIDIA, full metrics); WDDM GPU performance counters for AMD/Intel (utilization + VRAM) |
| Top processes | `ps -eo … --sort=-%cpu` / `--sort=-rss` | `ps -Ao … -r` / `-m` | `Get-Process` (two-sample CPU delta) |

Commands run with a 3-second timeout on a dedicated SSH connection (10 s on Windows to absorb PowerShell start-up). Windows commands are batched into a single PowerShell 5.1 invocation per poll and sent as `-EncodedCommand`, so they work with either cmd.exe or PowerShell as the OpenSSH default shell — nothing is installed and no admin rights are required. GpuTerm detects the remote OS and available GPU tools per host and shows a vendor tag on every card; `intel_gpu_top` needs root or `CAP_PERFMON`, and Apple GPU power/temperature would need root `powermetrics`, so they show as n/a. If no GPU source is present, the GPU section reports unavailable while everything else keeps working.

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
│    • Terminal      – PTY shell, dedicated connection per terminal cell │
│    • Telemetry     – own connection, auto-reconnect with backoff       │
│    • SFTP ops      – pooled per-session "operations" connection        │
│    • Bulk transfer – dedicated connection per file, cancellable        │
└────────────────────────────────────────────────────────────────────────┘
```

Long-running work is isolated: blocking SSH I/O runs on `spawn_blocking` threads so the UI never freezes, and each concern (shell / telemetry / transfers) fails independently.

**Data locations** (`%APPDATA%\GpuTerm` on Windows, `~/Library/Application Support/GpuTerm` on macOS, `~/.config/GpuTerm` on Linux):

| Location | Contents |
| --- | --- |
| `sessions.json` | Session profiles — host, port, username, key *path* only |
| `known_hosts.json` | Approved SHA-256 host key fingerprints |
| `app_settings.json` | UI preferences such as the last local SFTP directory |
| `credentials.enc` | Versioned Argon2id parameters plus an AES-256-GCM encrypted and authenticated credential payload |
| `credential_index.json` | Non-secret session ids used only to show which profiles have a saved vault entry |

Passwords and key passphrases are **never written in plaintext**. Private key contents are never copied into GpuTerm's configuration files.

## Development

```bash
npm run test                                    # frontend tests (Vitest)
cargo test --manifest-path src-tauri/Cargo.toml # backend tests
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets  # lints
npm run build                                   # TypeScript + Vite production build
```

<details>
<summary>Project layout</summary>

```
src/                    React frontend
  components/           TerminalPane, SftpBrowser, RemoteTelemetryBar, popovers…
  stores/               Zustand stores (session, transfers)
  utils/                Shared formatters, terminal buffer, disk priority,
                        WebKit Hangul IME workaround
src-tauri/src/ssh/      Rust backend
  terminal.rs           PTY shell + UTF-8 safe reader
  system_monitor.rs     Telemetry loop, OS detection, Linux parsers
  macos_monitor.rs      macOS collectors (sysctl, vm_stat, ioreg)
  windows_monitor.rs    Windows collectors (PowerShell CIM, WDDM GPU counters)
  gpu_monitor.rs        GPU tool probing + vendor parsers
  resource_details.rs   On-demand CPU/RAM/GPU detail collection
  sftp.rs               Transfers, cancellation, SFTP commands
  session.rs            Connections, host keys, profiles, connection pool
```

</details>

### Releases & CI

Pushing a `v*` tag runs the [Release Build workflow](.github/workflows/release.yml), which creates the prerelease from [RELEASE_NOTES.md](./RELEASE_NOTES.md), builds the Windows `.exe` (NSIS), Debian `.deb`, and macOS `.dmg` on GitHub-hosted runners, and attaches them to the tag. The macOS job signs every nested code object inside-out, deep-signs and verifies the final app, builds the DMG from that verified bundle, mounts it, and verifies the enclosed app again. A `SHA256SUMS.txt` file is published after all three installers succeed. The workflow can also be dispatched manually for an existing tag from the Actions page.

## FAQ

<details>
<summary><b>Does GpuTerm install anything on my servers?</b></summary>

No. Every metric is collected by running one-shot, read-only standard commands over SSH (`nvidia-smi`, `cat /proc/...`, `sysctl`, PowerShell `Get-CimInstance`, …). Nothing is copied to, installed on, or left behind on the remote host.

</details>

<details>
<summary><b>Where are my passwords stored?</b></summary>

Passwords and key passphrases are stored in `credentials.enc`. GpuTerm derives a 256-bit key from your master password with Argon2id (64 MiB, 3 iterations) and encrypts the complete payload with AES-256-GCM and a fresh random nonce on every write. The master password and key are kept only in memory for the current run; `sessions.json` contains host metadata and the *path* to a private key, never a secret. Deleting a profile also deletes its vault entry.

Older Keychain/Credential Manager/Secret Service entries are not imported or accessed. After upgrading, enter each SSH password again. If you forget the master password, reset the vault; profiles remain, but saved passwords must be entered again. See the [data locations](#architecture) table.

</details>

<details>
<summary><b>Do I need root/admin on the remote host?</b></summary>

No for all core metrics. A few extras need elevation and simply show n/a without it: per-core CPU and GPU power/temperature on macOS (`powermetrics`), process owners on Windows, and `intel_gpu_top` on Linux (root or `CAP_PERFMON`).

</details>

<details>
<summary><b>Which remote OSes are supported?</b></summary>

Linux, macOS (Apple Silicon included), and Windows with OpenSSH Server — see the [support matrix](#what-you-can-monitor). The remote OS is auto-detected per connection; WSL counts as Linux, and MSYS/Cygwin/Git-Bash shells on Windows are correctly detected as Windows.

</details>

<details>
<summary><b>Why does my OS warn me when installing?</b></summary>

Beta installers do not carry a trusted publisher/Developer ID signature or Apple notarization. The macOS bundle is fully ad-hoc signed for integrity, but this does not establish publisher trust. See the [installation warning](#installation) for the one-time SmartScreen/Gatekeeper steps, or build from source.

</details>

<details>
<summary><b>Can I use GpuTerm at work / in a commercial product?</b></summary>

GpuTerm is free for personal and noncommercial use under [PolyForm Noncommercial 1.0.0](./LICENSE). Commercial use (including shipping paid products built on this source) is not permitted under that license — contact the maintainer about a commercial license.

</details>

## Troubleshooting

| Symptom | Check |
| --- | --- |
| SmartScreen / Gatekeeper blocks the app | Expected for a beta without a trusted publisher signature/notarization — see the [installation warning](#installation) |
| `tauri:dev` fails on Windows | VS Build Tools 2022 (C++ workload) + WebView2 Runtime installed, then restart the terminal |
| `cargo` not found | Install via [rustup](https://rustup.rs), reopen the terminal (`%USERPROFILE%\.cargo\bin` on PATH) |
| SSH auth fails | Verify host/port/user/credentials; confirm the server allows the auth method |
| Pressing multiple keys shows `Terminal stream failed: transport read` | Fixed in v1.1.3-beta by synchronizing libssh2 and TCP nonblocking modes for direct and ProxyJump terminals — update the app |
| Master password is rejected or forgotten | Check the password, or choose **Reset vault**. Profiles are kept, but all saved SSH passwords are deleted and must be entered again |
| Host key mismatch | Verify the server fingerprint out-of-band, then remove the stale entry from `known_hosts.json` |
| GPU shows unavailable | Confirm a GPU tool is installed (`nvidia-smi`, `rocm-smi`, `xpu-smi`, or `intel_gpu_top`); other metrics still work regardless |
| Windows remote shows “The system cannot find the path specified” | Fixed in v1.0.9-beta — older builds misdetected Windows hosts that have a `uname` port on PATH as Linux; update the app |
| Korean input splits into jamo in the terminal | Fixed for macOS/WebKit clients — update to the latest release |

## Roadmap / Known limitations

- Keyboard-interactive SSH authentication is not implemented
- Recursive directory upload/download and transfer resume are not implemented
- `known_hosts.json` uses SHA-256 fingerprints, not the OpenSSH known_hosts format
- Telemetry supports local and remote Linux, macOS (Apple Silicon included), and Windows hosts; Apple GPU power/temperature and per-core CPU usage need root `powermetrics` and are not shown
- GPU monitoring uses `nvidia-smi`, `rocm-smi`, `xpu-smi`, `intel_gpu_top`, macOS `ioreg`, or Windows WDDM performance counters (AMD support on Linux currently targets `rocm-smi`)
- Windows remotes: requires Windows PowerShell 5.1+ (preinstalled); load averages don't exist and show as n/a; AMD/Intel GPUs report utilization and dedicated VRAM only (no power/temperature, needs Windows 10 1709+ with a WDDM 2.x driver); process owners and GPU process command lines need elevation and fall back to n/a / process names; `quser` is missing on Home editions, so the Users section stays empty there; hybrid iGPU+dGPU hosts show both cards (counters are attributed by adapter LUID from the DirectX registry, falling back to a positional heuristic if that key is unavailable)
- macOS installer currently targets Apple Silicon only (Intel Macs: build from source)

Issues and pull requests are welcome — please run the test suites above before submitting.

## License

[PolyForm Noncommercial 1.0.0](./LICENSE) © GpuTerm contributors. **Free for personal and noncommercial use; commercial use is not permitted** — see the [license](./LICENSE) or contact the maintainer for a commercial license. Built with [Tauri](https://tauri.app), [React](https://react.dev), [xterm.js](https://xtermjs.org), and [ssh2](https://crates.io/crates/ssh2); those third-party components remain under their own (open-source) licenses.
