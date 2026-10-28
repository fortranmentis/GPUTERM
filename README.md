<div align="center">

# GpuTerm

**The all-in-one SSH/SFTP desktop client for GPU servers.**

Terminal, file transfers, and real-time CPU · RAM · Disk · GPU telemetry (NVIDIA / AMD / Intel / Apple Silicon) — in a single native window.

[![Release](https://img.shields.io/github/v/release/fortranmentis/GPUTERM?include_prereleases&label=release&color=2ea44f&cacheSeconds=3600&r=1.3.1)](https://github.com/fortranmentis/GPUTERM/releases)
[![Release Build](https://github.com/fortranmentis/GPUTERM/actions/workflows/release.yml/badge.svg)](https://github.com/fortranmentis/GPUTERM/actions/workflows/release.yml)
[![Downloads](https://img.shields.io/github/downloads/fortranmentis/GPUTERM/total?color=8b5cf6&cacheSeconds=3600&r=1.3.1)](https://github.com/fortranmentis/GPUTERM/releases)
[![License: PolyForm Noncommercial](https://img.shields.io/badge/license-PolyForm%20Noncommercial%201.0.0-blue)](./LICENSE)
[![Built with Tauri](https://img.shields.io/badge/Tauri-2-FFC131?logo=tauri&logoColor=white)](https://tauri.app)
[![React](https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=white)](https://react.dev)
[![Rust](https://img.shields.io/badge/Rust-stable-DEA584?logo=rust&logoColor=white)](https://www.rust-lang.org)

[Website](https://fortranmentis.github.io/GPUTERM/) · [English](./README.md) · [한국어](./README.ko.md)

<img src="docs/screenshots/main.png" alt="GpuTerm main window: session sidebar with jump-host support, SSH terminal, SFTP browser, and the telemetry bar" width="850" />

</div>

---

Working on a remote GPU box usually means juggling an SSH client, an SFTP tool, and a second terminal running `watch nvidia-smi`. **GpuTerm replaces all three.** Connect once and get an xterm.js terminal, a drag-and-drop SFTP browser, and a live telemetry bar that polls CPU, memory, disk, logged-in users, and every GPU on the host — NVIDIA, AMD, Intel, or Apple Silicon, on Linux, macOS, and Windows remotes alike — over its own SSH channel, so monitoring never blocks your shell.

Almost nothing ever touches your servers: every metric comes from one-shot standard commands (`nvidia-smi`, `/proc`, `sysctl`, PowerShell CIM, …) over SSH, and no admin/root rights are required for the core metrics. The [three narrow exceptions](#faq) are an oversized Windows collector script, the opt-in Claude status-line helper, and LLM runtime monitoring — which runs no remote command at all.

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
- **Collapsible host selector** — hide the sidebar for more workspace and reopen it from the top-left button; full profile fields appear for **New** and for editing a saved profile (pencil button), while saved profiles otherwise show a connect-time credential prompt
- **ProxyJump** — tunnel through a saved profile as a bastion (per-key-type host verification along the way)
- **OpenSSH config import** — preview `~/.ssh/config`, resolve `Host`, `Hostname`, `User`, `Port`, `IdentityFile`, `ProxyJump`, and forwarding rules, then select the profiles to import without overwriting collisions silently
- **Built-in port forwarding** — save or start local (`-L`), remote (`-R`), and SOCKS5 dynamic (`-D`) rules, with live state and byte counts in the profile panel
- **Native local terminal** — open the current machine without SSH and use the same terminal splits and monitoring UI
- Password, private key (with passphrase), SSH agent, keyboard-interactive, and GpuTerm-managed key authentication — one-time codes (Google Authenticator, Duo) and PAM password prompts are asked in a dialog, including for each jump host
- UTF-8 safe streaming — multibyte characters (한글, 日本語, emoji) survive chunked reads
- **CJK input works correctly** — Korean IME composition in the terminal is handled with the same backspace-rewrite protocol native terminals use, fixing the jamo-splitting bug in WebKit-based webviews
- **Scrollback search** — `Ctrl`/`Cmd`+`F` opens an in-pane find bar with match count, `Enter`/`Shift`+`Enter` to step through hits, `Escape` to return focus to the shell
- **Saved command snippets** — keep global or host-scoped commands in the terminal toolbar; multiline snippets use bracketed paste when the shell advertises it
- **Terminal appearance and shortcuts** — choose among five themes, desktop monospace fonts, and an 8–28 px saved font size; the settings popover lists platform-safe split, pane, search, clear, and zoom shortcuts
- **Disconnect is visible and recoverable** — a pane whose shell or channel ends shows the reason the backend reported and a **Reconnect** button instead of silently refusing input
- **Automatic recovery after sleep or network loss** — unexpected disconnects retry with bounded backoff when the machine comes online, the window regains focus, or a suspended heartbeat resumes; authentication and host-key failures stop for review. An idle connection is asked for a reply after 15 s and right after the computer wakes; responsive-probe servers allow up to 8 s for a reply, with a keepalive fallback for servers that do not answer probes
- MOTD and early output are buffered and replayed, never lost to connection races
- Serialized burst input plus a fully nonblocking SSH/TCP transport keeps simultaneous keys, modifier chords, and key repeat responsive without racing the terminal reader
- Terminal writes avoid libssh2's incoming-data flush path, while writes, remote PTY resize, and SSH keepalive retry nonblocking operations without packet interleaving
- IPv4 and IPv6 literals are accepted, every resolved address is tried with a timeout, and remote PTY resize plus SSH keepalive also cover ProxyJump tunnels

### 📁 SFTP Browser
- **Checkbox multi-selection** — per-row and tri-state header checkboxes, Shift-click ranges in displayed sort order, and Ctrl/Cmd+A in the focused file panel. Upload, Download and Delete show selected counts `(N)`
- Side-by-side remote/local panels with a draggable divider for adjusting their vertical ratio
- **Sortable remote columns** — click Name, Type, Size, or Modified; click again to reverse direction while folders stay first and missing metadata stays last
- **Decimal file sizes** — remote/local listings and transfer progress use consistent SI `B / KB / MB / GB / TB` units
- Drag-and-drop upload & download for **files and complete directory trees**, with aggregate folder progress and cancellation
- **Native desktop drops and paste uploads** — drag files/folders from Explorer, Finder, or Nautilus, or paste URI-list items into the remote pane
- **Native desktop drag-out** — drag remote files, complete folders, or multi-selections from GpuTerm into Finder, Explorer, or a Linux file manager
- **Platform-aware drag interop** — GTK file URIs remain available until Linux file managers finish reading them, while native macOS/Linux and Windows drop coordinates are normalized correctly on scaled displays
- Streaming 1 MiB chunked transfers with a progress queue and **per-item cancellation**
- Uploads write to a unique remote `.gputerm-part` file and rename only after completion, preserving an existing destination across cancellation or transfer failure
- Downloaded files are written to temporary files and atomically renamed — no partial files are exposed
- Remote rename and drag-to-move, Replace / Keep existing / Skip folder-merge choices, delete, contextual mkdir beside Open, and a native OS folder picker
- Resizable split between terminal and SFTP panes (persisted across launches)
- **Collapsible SFTP panel** — close it with the directional panel button and restore it from the top-right; the terminal immediately expands into the freed width
- **Responsive narrow layout** — metadata columns collapse progressively and transfer controls remain accessible when the SFTP pane is resized to its minimum width

### 📊 Live Telemetry
- Bottom status bar polling CPU, RAM, disk, logged-in users, and GPUs every 1–10 s — on **local or remote Linux, macOS, and Windows hosts**
- **Collapsible monitoring bar** — close it independently and restore it from the bottom-right; visibility is remembered across launches
- **NVIDIA, AMD, Intel, and Apple Silicon** GPUs are auto-detected per host; every card carries a vendor tag
- **Hybrid iGPU + dGPU hosts show both cards** — Linux supplements vendor tools with DRM/sysfs adapter discovery, while Windows attributes counters by DirectX LUID and keeps idle adapters visible even before WDDM creates an activity counter
- **AI DASH for AGY, Codex, and Claude Code** — the bottom card shows every available 5-hour and weekly balance at once in compact gauges, while the detail view retains context, tokens, and aggregate CPU/RAM across each CLI's complete child-process tree. AGY uses the precise percentage printed beside each `/usage` bar, lists the models in each group, and plots their in-memory 24-hour trend
- **Cross-platform local AI quotas** — macOS Claude setup uses built-in `osascript` without Python; Windows Codex/AGY probes reuse observed native executables and support npm `.cmd` shims, while Windows Claude uses a consistent User Profile path
- **CPU, DIMM, and drive temperatures** where the host actually exposes them — Linux reads hwmon (`coretemp`/`k10temp`/`zenpower` for the CPU die, `jc42`/`spd5118` for DDR3–DDR5 DIMMs, `nvme`/`drivetemp` for drives), Windows reads the ACPI thermal zone plus SMART drive temperatures. Thresholds are per class (a Zen 4 CPU at 88 C is normal; an NVMe at 72 C is not), and a sensor's own `tempN_max`/`tempN_crit` wins over any built-in constant
- **A host that cannot report a temperature says so, and says why** — WSL2, containers, and most VMs pass no sensors through, and macOS exposes none without root. Those categories name the missing driver or interface in the detail popover instead of showing a misleading `0`, and the bar card simply omits the reading
- **Bounded disk summaries** — long mount paths are ellipsized inside the card without hiding utilization, with the complete path available on hover
- Click any section for a **draggable, resizable detail popover** whose tables expand with the window: per-core CPU usage, top processes, VRAM/power/temperature per GPU, a gauge per CPU/DIMM/drive temperature sensor, full mount list
- **Pop any detail view out into its own OS window** — it refreshes independently and closes with its session
- **Usage history** — CPU, memory, the fullest disk, and each GPU's utilization, VRAM, temperature, and power are kept for the last hour in 5-second buckets; popovers show a 15-minute trend and detached windows switch between 15 minutes and 1 hour
- **Alerts and notifications** — the bell in the monitoring bar sets thresholds for GPU temperature, VRAM, memory, each disk, an idle GPU still holding memory, and an LLM runtime turning critical. A reading must stay over its threshold for a chosen time, clears only below a lower level, and is notified once (system notification, or an in-app message while GpuTerm is in front)
- **End a process** — CPU, memory, and GPU process tables can end a process after showing its owner and command; GpuTerm re-checks that the PID still belongs to the same process right before signalling, offers a forced stop if it keeps running, and never uses `sudo`
- Remote telemetry runs on a dedicated SSH connection with automatic reconnect; local telemetry executes collectors directly without SSH
- **Quiet Windows local monitoring** — PowerShell collectors run without allocating console windows and emit UTF-8 text, so localized device and volume names remain parseable
- Hosts without any GPU gracefully fall back to system-only metrics

### 📦 Containers and SQL monitoring

- **Containers** appears for connected remote Linux hosts in a system-inclusive monitoring mode when Docker, Podman, nerdctl, kubectl, k3s or microk8s is detected. It reads the first available container runtime and Kubernetes client on PATH, including container CPU/memory and node/deployment/pod state.
- Start, restart, stop or kill a container, or roll out a deployment restart, only after confirmation. Commands use the SSH user's socket permissions and Kubernetes context/RBAC; GpuTerm never adds sudo. The pod view is bounded to 300 entries; absent runtimes hide the card.
- **SQL** reads PostgreSQL and MySQL/MariaDB through `psql` or `mysql`/`mariadb` on a connected Linux/macOS SSH server. Register an SSH profile and a DB address as seen from that server. Connections and commit/query rates, lock waits, longest queries, cache hit and replication lag depend on engine and privileges. Eligible instances are polled every 10 seconds; monitoring does not execute application queries or change DB configuration.
- DB passwords stay in the local encrypted vault. A single-line password reaches the collector on stdin and is written to a mode-600 temporary `PGPASSFILE`/client-options file with exit cleanup. It is not embedded in command arguments or exported as an environment value. Abrupt termination can prevent cleanup.
- Linux host labels and detached-window titles use `/etc/os-release` (or `/usr/lib/os-release`), including Fedora, openSUSE, Arch and Gentoo. Hosts without readable utmp data use `loginctl` when available. These additions concern monitored servers; release installers remain EXE, Apple Silicon DMG and DEB.

### 🌐 Remote web services

- **Web services** in the session sidebar finds TensorBoard, Jupyter, MLflow, Ray Dashboard, Streamlit, Gradio and code-server on connected SSH hosts. Linux uses `ss` (with a `/proc/net/tcp` fallback), macOS uses `lsof`, and Windows uses PowerShell connection/process information. Linux also inspects up to 50 accessible Docker containers for exposed TCP ports.
- All connected remote hosts are scanned every 20 seconds. After the initial scan, newly detected known services can show an **Open** toast; **Notify when one starts** toggles this per-device preference. Unknown listeners stay under **Other listening ports** and are not announced.
- **Open** creates or reuses a temporary loopback-only SSH forward, preferring the same local port or choosing a free one. The pin button saves a forward for future connections; it does not save the Jupyter login token. Runtime-file tokens are included when available so Jupyter opens at its base URL.
- **Cmd/Ctrl-click terminal HTTP(S) links**, including wrapped URLs, to open the default browser. Plain HTTP links to localhost, wildcard addresses or the SSH host's own name use a local SSH forward; other links, including HTTPS, open unchanged.
- Discovery is best-effort: permissions, runtime paths, process ownership, Docker networking and port-based classification limit what is found. An exposed container port or an unknown listener does not prove a healthy HTTP service; login may still be needed. Local-terminal automatic discovery is not provided, and GpuTerm does not start servers or install remote agents.
- MCP clients can use the read-only `list_web_services` tool. Its returned paths omit Jupyter login tokens; the in-app browser-opening flow retains them when available. MLflow backend/artifact connection URIs are omitted from both UI and MCP metadata because they can contain credentials.

### 🤖 LLM runtime monitoring
- Register any number of **Ollama, vLLM, SGLang, Hugging Face TGI, llama.cpp, OpenAI-compatible, and NVIDIA Triton** servers by URL; each is polled independently of your SSH sessions, so the card works with no terminal connected
- **Poll directly, or through an SSH tunnel** — pick a saved SSH profile as the instance's *Reach through* and the address is resolved on that host, so a runtime bound to its own `127.0.0.1` needs nothing exposed on the network
- **Read-only.** GpuTerm calls only each adapter's health, catalog, metrics, or statistics endpoints. It never sends an inference request, downloads or deletes a model, or restarts a server
- **Ollama** — running and installed models, model size, VRAM-resident size and share, estimated non-VRAM residency, configured maximum context, and how long a loaded model stays in memory
- **vLLM** — running/waiting/swapped requests, server-wide KV cache usage and headroom, prefix-cache hit rate, prompt and generation tokens per second, request rate, preemptions, and TTFT/latency/queue percentiles interpolated from Prometheus histogram buckets
- **Other serving engines** — SGLang, TGI, and llama.cpp expose the health, model, queue/cache/token, and latency metrics their endpoints publish; Triton reports readiness and per-model inference statistics; generic OpenAI-compatible servers provide model catalog and reachability
- **Nothing is faked.** A metric this build of the runtime does not publish is shown as *not supported*, and a value that has not been read yet is shown as `—`. Neither is ever displayed as `0`
- **Counter restarts are handled** — a counter that goes backwards produces a null rate for that interval and a "server was probably restarted" event, never a negative throughput
- API keys go straight into the encrypted `credentials.enc` vault. They are never returned to the webview, never written to `llm_instances.json`, and masked out of error text
- Per-instance poll interval and timeout, enable/disable, a connection test that saves nothing, exponential backoff on repeated failure, and an in-memory trend chart that switches between the last 15 minutes and the last hour

### 🔌 AI assistants (MCP) and in-app updates
- **Built-in MCP server** — turn on **MCP** at the bottom of the sidebar and Claude Code, Claude Desktop, Codex or Cursor can read the hosts you have connected: live telemetry, the last hour of history, free GPUs across hosts, GPU/top processes, containers, tmux sessions, directory listings, LLM runtimes and firing alerts. See [MCP server for AI assistants](#mcp-server-for-ai-assistants)
- **Actions only with your approval** — with **Allow actions** on, `run_command`, `read_file` and `kill_process` each open a confirmation in GpuTerm (Deny by default). Loopback only, a per-start token, per-host hiding and an audit log
- **In-app updates** — GpuTerm checks GitHub for a newer release and installs it from a signed update bundle after you press **Update**. See [Updates](#updates)

### 🔐 Security by default
- **SSH key manager** — open **SSH keys** to generate Ed25519, RSA 3072/4096, or ECDSA P-256/P-384/P-521 keys. Copy/save public keys, rename/delete local keys, and export private keys with a passphrase or by explicitly choosing **Save without a passphrase**. Private keys live in the vault; `keys.json` contains public metadata and profile assignments only.
- Choose **Sign in with key** on a profile and **Install on server** to install and verify it before assignment. Normal sign-in tries the profile's own managed key, then the selected manager key, then the existing key-file/password/agent flow. Deleting a manager key locally does not revoke it on servers.
- **GpuTerm-managed SSH keys** — **SSH key → Set up key** on a saved profile creates an Ed25519 key, installs it in `authorized_keys` (700/600 permissions, or `administrators_authorized_keys` for Windows administrators), and verifies a new-key-only sign-in before saving it. The private key stays in the encrypted vault and goes to libssh2 from memory. **Export…** writes passphrase-encrypted OpenSSH files (8+ characters); **Replace** / **Remove** request old-key revocation, subject to server access and permissions
- Passwords, key passphrases, and managed private keys are stored only in the local `credentials.enc` vault: Argon2id derives a 256-bit key from your GpuTerm master password, and AES-256-GCM encrypts and authenticates the complete credential payload
- By default the master password and derived key stay in memory for the current run. Optional **Touch ID / Windows Hello** persists a copy of the derived vault key using OS account protection: macOS login Keychain or a Windows DPAPI-protected file. The master password is not stored and `sessions.json` contains no private keys/passwords
- **Biometric unlock is opt-in.** Enable its sidebar toggle after opening the vault. The app tries once at startup; cancellation/failure leaves master-password unlock available. Linux stays password-only. GpuTerm verifies the user before reading/unprotecting the saved key; this ad-hoc build does not cryptographically bind it to biometric hardware or exclude access by other authorized processes under the same OS account
- Saved-session password fields show a secure mask while keeping the actual secret out of the webview; leaving the field blank reuses the vault entry
- Trust-on-first-use host key prompt with SHA-256 fingerprint; a changed key shows the old and new fingerprints and is replaced only after explicit confirmation
- Profiles referenced by ProxyJump or an LLM tunnel cannot be deleted; successful deletion tears down terminals, forwards, monitoring connections, and in-memory credentials before local UI removal
- Restrictive Tauri Content Security Policy in production and development

> **Credential storage:** since 1.1.2-beta, passwords use the Argon2id/AES-256-GCM vault rather than old OS credential entries, which are not imported. 1.3.6-beta biometric unlock is an explicit opt-in exception: Keychain/DPAPI stores a derived vault-unlock key, not individual SSH/DB passwords. Leave it off for master-password-only unlock.

## What you can monitor

| Metric | Linux | macOS (Apple Silicon) | Windows (OpenSSH Server) |
| --- | :-: | :-: | :-: |
| CPU model · cores · usage | ✅ | ✅ (P/E core split) | ✅ |
| Load average | ✅ | ✅ | — (doesn't exist on Windows) |
| Memory + swap | ✅ | ✅ (Activity Monitor semantics) | ✅ (page file as swap) |
| Disks / mounts | ✅ | ✅ | ✅ (fixed drives) |
| Logged-in users | ✅ `who` | ✅ `who` | ✅ `quser` (absent on Home editions) |
| Containers / Kubernetes | ✅ remote hosts | — | — |
| PostgreSQL / MySQL / MariaDB | ✅ remote CLI | ✅ remote CLI | — |
| NVIDIA GPU (util · VRAM · power · temp · processes) | ✅ `nvidia-smi` | — | ✅ `nvidia-smi` |
| AMD GPU | ✅ `rocm-smi` or `amd-smi` (util · VRAM · power · temp · processes, where exposed) | — | ◐ WDDM counters (util + VRAM) |
| Intel GPU | ◐ `xpu-smi` / `intel_gpu_top` | — | ◐ WDDM counters (util + VRAM) |
| Apple GPU | — | ◐ util + memory (power/temp need root) | — |
| CPU temperature | ✅ hwmon (`coretemp`/`k10temp`/`zenpower`) or `/sys/class/thermal` | — (needs root `powermetrics`) | ◐ ACPI thermal zone, firmware-dependent, **not the die** |
| DIMM temperature | ◐ only with `jc42` (DDR3/DDR4 ECC) or `spd5118` (DDR5, kernel 6.10+) | — | — (no WMI/CIM interface) |
| NVMe / drive temperature | ✅ `nvme` / `drivetemp` hwmon | — | ✅ SMART via `GetReliabilityCounter` |
| AGY · Codex · Claude Code process trees | ✅ `ps` | ✅ `ps` | ✅ CIM + `Get-Process` |
| Ollama / vLLM / SGLang / TGI / llama.cpp / OpenAI-compatible / Triton | ✅ HTTP | ✅ HTTP | ✅ HTTP |
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
| Linux (x64, portable) | `GpuTerm_x.y.z_amd64.AppImage` | `chmod +x` and run; updates itself in-app |

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

### Updates

From 1.3.8-beta on, GpuTerm checks GitHub for a newer release shortly after launch and every six hours. When one is out, a dialog shows its release notes with **Update**, **Later** and **Skip this version**. **Update** downloads the new build, checks its signature against the public key built into the app, installs it, and offers **Restart now**. **Check for updates** at the bottom of the sidebar runs the same check by hand and also shows the installed version.

- The first build that includes the updater has to be installed by hand once; every later release can then be installed from inside the app.
- **Debian/Ubuntu `.deb` installs** belong to the package manager, so the dialog opens the release page instead of installing. The AppImage updates itself.
- Updates are signed with GpuTerm's own update key, separately from the OS code signing described above; an update whose signature does not match is refused.

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
2. **Create a profile** — enter host, port, username, and a password or private key path in the sidebar. Press **New** to start a fresh profile, **Save** to keep it (a typed password is stored in the vault), and the pencil beside a selected profile to edit it. To route through a bastion, pick any saved profile as the **Jump host**.
3. **Connect** — on first contact GpuTerm shows the server's SHA-256 host key fingerprint and asks for confirmation before trusting it. Connect as many servers as you like; connected profiles show a green dot, and clicking one switches the whole view to that session.
4. **Split terminals** — use the columns button to open another shell for the focused session, or the **+** button to add a different saved session. Choose left/right/top/bottom placement and the new pane's initial size before adding it.
5. **Work** — type in the terminal, drag or paste files into the remote SFTP panel, and watch live metrics in the bottom bar. Click CPU / RAM / Disk / GPU / Users for detail popovers you can drag around, resize, or pop out into separate windows with the ↗ button.
6. **Register an LLM runtime (optional)** — click **LLM RUNTIME** in the monitoring bar, then **Add**. This card is not tied to a session, so it works before you connect anything.

<details>
<summary>Set up a GpuTerm-managed SSH key</summary>

- Unlock the vault, save/select a remote profile and enter working password/key/agent credentials. Open **SSH key → Set up key**; this explicitly changes the remote authorized-key file. ProxyJump hops keep their own credentials.
- After setup succeeds, normal connections try the managed key first. Inspect its public key and SHA-256 fingerprint in the panel. Setup verification permits only the new key on the final host, so a password or agent cannot disguise rejection.
- Use **Export…** for a backup: choose and repeat an 8+ character passphrase, then save an encrypted OpenSSH private key and adjacent `.pub` file. Keep both the exported file and passphrase safe.
- **Replace** verifies the new key before attempting old-key removal. **Remove** removes it from the server by default; unticking the server option forgets it locally only. Profile deletion/vault reset also leaves the server's public key in place. Confirm server-side revocation if cleanup reports a failure.

</details>

<details>
<summary>Terminal split controls</summary>

- The **columns** button opens another independent PTY shell for the currently focused session.
- The **+** button lists saved profiles. A live session is added immediately; a disconnected profile asks for its password/key passphrase (and jump-host password when applicable) before connecting.
- The **terminal** button lists the tmux sessions running on the focused server. Clicking one attaches to it in a new pane, so detaching with `Ctrl-B d` leaves you at a shell instead of closing the pane. The form below creates a session — leave the name blank to let tmux number it — and each row's **×** kills that session.
- The **snippet** button stores reusable commands globally or for the focused profile. The **appearance** button selects theme, font, and font size and shows the active platform's terminal shortcut table.
- Up to four cells can be nested horizontally and vertically. Choose a 20–80% initial size, then drag any divider to adjust the ratio. Each mixed-session cell displays its profile name.
- Clicking a cell makes that session active for SFTP and telemetry while preserving the split layout. The cell's **×** button closes that terminal pane.
- Unexpected transport loss is retried automatically after network or sleep recovery. A deliberate disconnect, authentication failure, or changed host key is never retried behind the user's back.

The tmux picker is optional and does not install anything: `tmux` must already be available on the local or remote Unix host. GpuTerm checks the login `PATH` plus common Homebrew, `/usr/local`, user-local, and Snap locations. Native Windows hosts are reported as unsupported; WSL can be used through its SSH server as a Linux host.

</details>

<details>
<summary>SSH config import and port forwarding</summary>

- Use the **Import** icon beside **New** to preview `~/.ssh/config`. The adjacent key icon opens **SSH keys**. Both compact buttons retain accessible names and hover tooltips so the **New** action has room in the sidebar header. GpuTerm follows `Include` (relative to `~/.ssh`, with globs such as `conf.d/*`), expands `~/` and `%d` identity paths, resolves `ProxyJump`, and identifies profile-name collisions before importing only the selected entries.
- Expand **Port forwards** on a saved profile to add local (`-L`), remote (`-R`), or SOCKS5 dynamic (`-D`) rules. Saved rules start with the profile; **Start once** creates a connection-only rule.
- Bind addresses and ports are validated before saving. The live list reports starting/running/error state and byte counts; stopping a rule leaves the SSH terminal connected.
- Ports below 1024 are accepted by the editor, but the operating system may still require elevated privileges to bind them.

</details>

<details>
<summary>Workspace panel controls</summary>

- The host selector, SFTP browser, and monitoring bar each have a directional close button in their header.
- Reopen a hidden panel from its matching workspace edge: host selector at the top-left, SFTP at the top-right, and monitoring at the bottom-right.
- Each panel's open/closed state is saved locally. Hiding SFTP expands terminal width; hiding monitoring expands terminal height.

</details>

<details>
<summary>SFTP transfer details</summary>

- Drop multiple files or folders at once; each top-level item becomes an independent queue item with progress, status, and error reporting. Folder contents are transferred recursively.
- Running transfers can be canceled individually from the queue.
- If the target exists, GpuTerm asks before replacing a file or merging into an existing folder.
- Rename an item in place, or drag remote selections onto a remote directory to move them without downloading. A move is rejected if the destination name already exists.
- Uploads are committed by remote rename from a hidden `.gputerm-part` path. If the final rename cannot be completed even after the SFTP v3 overwrite fallback, the recovery part path is kept and shown instead of deleting the only complete copy.
- Files and folders dragged from Explorer, Finder, or Nautilus use the native desktop path payload; items copied in Nautilus and compatible file managers can also be pasted into the focused remote pane.
- To export remote items to another application, drag them toward a GpuTerm window edge. GpuTerm materializes them in a temporary local export and then starts a native OS copy drag. Large items may finish preparing after the pointer is released; drag them again once the transfer queue reports completion.
- On Linux, GpuTerm keeps the GTK `text/uri-list` provider alive until the native drag has ended so Nautilus and other file managers can complete their asynchronous file request. On Retina/scaled displays, native drop coordinates are converted per platform before the remote pane is selected.
- Click a local folder once to select it for transfer and double-click it to open it. The divider between the remote and local lists can be dragged or adjusted with the arrow keys.
- Click a remote column header to sort it. Name/Type start ascending, Size/Modified start descending, repeated clicks reverse the direction, and the current sort remains active while navigating or refreshing.
- At narrow SFTP widths, Modified and then Type are hidden automatically; Browse and transfer actions switch to icon-only controls with accessible labels rather than overflowing the pane.
- Symbolic links are rejected during recursive transfer to prevent cycles and unexpected traversal outside the selected tree.
- The last local directory is remembered across launches.

</details>

<details>
<summary>Telemetry configuration</summary>

- **Interval:** 1, 2 (default), 5, or 10 seconds — detail popovers poll on the same cadence.
- **Mode:** GPU + System, GPU only, or System only.
- **Ignore FS:** comma-separated filesystem types hidden from the disk summary (default: `tmpfs`, `devtmpfs`, `squashfs`, `proc`, `sysfs`, `cgroup`, `cgroup2`, `overlay`, `devfs`, `autofs`). The disk popover can temporarily reveal them.
- The interval, display mode and ignored filesystems are saved in `app_settings.json` and restored on the next launch.
- Mount points are prioritized `/` → `/home` → `/data` → `/mnt*` → `/media*` → drive letters → others; disks ≥ 80% are flagged warning, ≥ 90% critical. Long compact-card paths are ellipsized, and their full value remains available as hover text and in the disk detail view.
- **AI DASH:** the System modes include a 360 px summary card for AGY, Codex, and Claude Code. Its thin two-column gauges show every available 5-hour and weekly balance simultaneously; unsupported periods show `n/a`, expired windows show `reset`, and warning colors start at 25% and 10% remaining. Duplicate sessions share the newest account snapshot. The detail view keeps context and token data collapsed and includes descendants such as language servers, subagents, and background commands in CPU/memory totals. Codex uses account-wide `account/rateLimits/read` and labels its session-log fallback. Claude Code limits come from [Claude Code usage limits](#claude-code-usage-limits). AGY experimentally reads `/usage` in a hidden PTY every five minutes, preserves the Gemini and Claude/GPT model groups, and plots successful and failed samples over the last 24 hours without estimating missing balances.

</details>

<details>
<summary>Connecting an LLM runtime</summary>

All supported runtimes use the same flow: **LLM RUNTIME → Add**, pick the runtime, enter the address, and press **Test connection** before **Add**. Only `http` and `https` addresses are accepted; a trailing `/`, a path, or a query string is stripped, so `http://host:11434/` and `http://host:11434` are the same instance. The same runtime cannot be registered twice at the same address.

**Ollama** — the default address is `http://<host>:11434`. Ollama binds to `127.0.0.1` unless told otherwise, so to monitor it from another machine either start it with `OLLAMA_HOST=0.0.0.0:11434` (or set that in the service unit) and open the port, or leave it on loopback and use the SSH tunnel described below. No API key is needed. GpuTerm reads `/api/ps` every poll and `/api/tags` every five minutes.

**vLLM** — the default address is `http://<host>:8000`, i.e. the `--host`/`--port` given to `vllm serve`. If the server was started with `--api-key`, enter the same value in the **API key** field; GpuTerm sends it as `Authorization: Bearer …` on every request and stores it in the encrypted vault. `/metrics` is enabled by default; if the server runs with `--disable-log-stats` it publishes no serving metrics and GpuTerm reports them as unavailable rather than zero. GpuTerm reads `/health` and `/metrics` every poll and `/v1/models` every minute.

**SGLang, TGI, llama.cpp, OpenAI-compatible, and Triton** — choose the matching adapter so GpuTerm uses the server's native read-only endpoints. SGLang and llama.cpp use `/health`, `/v1/models`, and `/metrics`; TGI uses `/health`, `/info`, and `/metrics`; generic OpenAI-compatible servers use `/v1/models`; Triton uses `/v2/health/ready` and `/v2/models/stats`. llama.cpp requires `llama-server --metrics` for Prometheus metrics; otherwise the card explains that metrics are disabled rather than showing zero.

For an `https` address, the certificate is validated against the operating system trust store, so an internal CA already trusted by the machine works without extra configuration.

**Or leave the runtime on loopback and tunnel over SSH.** Set **Reach through** to a saved SSH profile instead of `Direct`. The address is then resolved *on that host*, so `http://127.0.0.1:11434` reaches a runtime bound to the remote machine's own loopback — no `0.0.0.0` binding, no firewall rule, no `netsh portproxy`, and the traffic is carried inside SSH. GpuTerm opens one SSH connection per profile (shared by every tunneled instance on that host) and forwards one loopback port per instance.

Two things to know before choosing it:

- The SSH host key must already be trusted. Press **Test connection** once — that is the only place the fingerprint prompt can appear, because the background poller has no way to ask. After trusting it, monitoring starts on its own within one poll interval.
- The forwarded loopback port stays open for as long as the app runs, and any process on your own machine can reach the remote runtime through it. This is exactly what `ssh -L` does and no worse, but it is a real difference from polling a LAN address.

`https` cannot be tunneled: the request would arrive at `127.0.0.1`, so the certificate would be checked against that address and could not match. GpuTerm refuses the combination when you save it rather than failing later.

</details>

<details>
<summary>Metrics no runtime reports, and why</summary>

| Not shown | Why |
| --- | --- |
| Ollama request count, queue depth, TTFT, tokens/s | `/api/ps` and `/api/tags` do not expose them. The per-request figures exist only in an inference response, and GpuTerm never sends an inference request |
| Ollama current context usage | Only the configured maximum is published. It is labeled *configured max context* for that reason |
| vLLM `num_requests_swapped` | Removed in newer vLLM. Shown as *not supported*, never as `0` |
| vLLM percentiles when the histogram is absent | Some builds omit `_bucket` series or use `--disable-log-stats`. Percentiles are left blank rather than invented |
| Ollama non-VRAM residency as RAM | `size - size_vram` is an estimate of CPU offloading, not a measurement of system RAM, and is labeled as such |

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
| GPU | `nvidia-smi` (NVIDIA), `rocm-smi --json`, or `amd-smi --json` where rocm-smi is missing or fails (AMD/ROCm), `xpu-smi` / `intel_gpu_top` (Intel), plus `/sys/class/drm/card*/device` for uncovered adapters | `ioreg -c IOAccelerator` (Apple GPU utilization, no root needed) | `nvidia-smi` (NVIDIA, full metrics); WDDM GPU performance counters for AMD/Intel (utilization + VRAM) |
| Temperature | `/sys/class/hwmon/hwmon*/{name,temp*_label,temp*_input,temp*_max,temp*_crit}` and `/sys/class/thermal/thermal_zone*/{type,temp}` | — (no unprivileged source) | `MSAcpi_ThermalZoneTemperature` in `root\WMI` (CPU zone), `MSFT_PhysicalDisk` + `GetReliabilityCounter` (drives) |
| Top processes | `ps -eo … --sort=-%cpu` / `--sort=-rss` | `ps -Ao … -r` / `-m` | `Get-Process` (two-sample CPU delta) |
| AI DASH | `ps -axo …`; provider metadata; Codex app-server quota lookup; optional AGY `/usage` PTY probe | same | `Win32_Process` + `Get-Process`; provider metadata, Codex account lookup, and optional AGY `/usage` ConPTY probe |

LLM runtime monitoring is the exception: it runs no remote command at all. GpuTerm issues read-only HTTP GETs from the desktop machine to the addresses you register. The adapter-specific endpoints are documented in [Connecting an LLM runtime](#usage); none sends an inference request or changes server state.

Temperature metadata — driver names, sensor labels, vendor thresholds — is probed once per connection and only the raw values are re-read each tick. Windows drive temperatures run as their own command at most every 30 seconds so a sleeping spindle is not woken by polling, and they are deliberately kept out of the batched PowerShell script: one slow drive there would set the CPU, memory, disk, and user errors at once, which is exactly the state the reconnect logic reads as a dead transport.

Commands run with a 3-second timeout on a dedicated SSH connection (10 s on Windows to absorb PowerShell start-up); Codex account reads use 5 seconds and the experimental AGY PTY probe uses 15 seconds. Windows commands are batched into a single PowerShell 5.1 invocation per poll and work with either cmd.exe or PowerShell as the OpenSSH default shell. Small scripts travel as `-EncodedCommand`; larger ones are uploaded over SFTP under a content-addressed name and run with `-File`, because base64 of UTF-16LE inflates a script by roughly 2.7x while cmd.exe — the OpenSSH default shell for exec requests — stops at 8,191 characters. Uploaded scripts live under `~/.gputerm/scripts`, are re-sent only when their contents change, and are pruned after seven days; nothing is installed and no admin rights are required. For a local Windows session, the same collectors use the system PowerShell directly with `CREATE_NO_WINDOW` and explicit UTF-8 text output, preventing polling consoles from appearing and preserving localized JSON fields. GpuTerm detects the remote OS and available GPU tools per host and shows a vendor tag on every card; `intel_gpu_top` needs root or `CAP_PERFMON`, and Apple GPU power/temperature would need root `powermetrics`, so they show as n/a. Linux DRM/sysfs supplies adapter identity and any driver-exported utilization/VRAM counters for GPUs not covered by a richer vendor collector. If no GPU source is present, the GPU section reports unavailable while everything else keeps working.

AI DASH monitoring is read-only except when the user explicitly presses Claude's **Set up** button. Process totals are collected every telemetry interval and session metadata at most every five seconds. Account quotas are kept separately from session context: every live session for one provider receives the same newest account snapshot. Codex queries `account/rateLimits/read` at most once per minute and falls back to the newest timestamped session-log record if the app-server lookup fails. While AGY is running, GpuTerm waits for a hidden PTY to become ready, sends `/usage` exactly once on first detection and then at most once per five minutes, and never sends a plain `usage` command. The precise bar percentage takes precedence over the rounded `remaining` line; model membership and reset countdowns are also normalized. GpuTerm keeps one point per five-minute bucket for the latest 24 hours (maximum 288) in memory only; reconnects in the same app run share that history, failures create visible graph gaps, and app exit discards it. Raw terminal output is discarded after parsing. Prompts, responses, tool arguments, credentials, and environment data are never serialized.

</details>

### MCP server for AI assistants

GpuTerm can act as an [MCP](https://modelcontextprotocol.io) server, so Claude Code, Claude Desktop, Codex, Cursor and other MCP clients can look at the hosts you have connected. Click **MCP** at the bottom of the sidebar, turn on **Allow MCP clients**, then copy the setup for your client from the same dialog. Claude Code, for example:

```bash
claude mcp add --scope user gputerm -- "/Applications/GpuTerm.app/Contents/MacOS/gputerm" --mcp-stdio
```

| Tools | What they do |
| --- | --- |
| `list_hosts`, `get_host_status`, `get_usage_history`, `find_free_gpus` | Hosts, the latest CPU/memory/disk/GPU reading, the last 15 or 60 minutes, idle GPUs across hosts |
| `list_gpu_processes`, `list_top_processes`, `list_containers`, `list_tmux_sessions`, `list_directory` | The same detail views as the telemetry popovers and SFTP browser |
| `get_llm_runtimes`, `get_active_alerts` | LLM runtime status and alerts that are currently firing |
| `run_command`, `read_file`, `kill_process` | Offered only with **Allow actions** on; every call opens a confirmation in GpuTerm (Deny is the default; no answer in two minutes is a denial) |

- Clients start `gputerm --mcp-stdio`, which relays to the running app over `127.0.0.1` with a random token that is regenerated each time the server starts and stored in a file only your user can read. Nothing listens on the network, and when GpuTerm is closed the tools just say so.
- Tools only see hosts that are **connected** in GpuTerm, and you can hide hosts from MCP in the same dialog. Saved passwords and keys are never exposed.
- Command output, file contents and file names are returned marked as untrusted data, because a remote file can contain text written to steer an AI assistant.
- Every call is recorded in `mcp-audit.log` in the GpuTerm data folder (rotated at 1 MiB).

### Claude Code usage limits

Claude Code never writes its 5-hour and 7-day usage limits to a session transcript — the status-line hook is the only place it publishes them. GpuTerm ships a status line that republishes that data for AI DASH and prints a usable status line of its own:

The easiest setup is **AI DASH → Claude Code → Set up**. GpuTerm backs up `settings.json`, installs the helper on that local or SSH host, and never replaces an unrelated custom status line. The commands below remain available for manual POSIX setup.

```bash
cp scripts/gputerm-claude-statusline.sh ~/.claude/gputerm-claude-statusline.sh && chmod +x ~/.claude/gputerm-claude-statusline.sh
```

Then add it to `~/.claude/settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "~/.claude/gputerm-claude-statusline.sh",
    "padding": 0
  }
}
```

The status line prints `Opus · ctx 8% · 5h 76% · wk 59% · $0.12`. It writes `~/.cache/gputerm/agent-status/claude/<session-id>.json` for that session, and — whenever the payload actually carries limits — refreshes `~/.cache/gputerm/agent-status/claude/account.json` with the account-wide 5-hour and weekly windows. GpuTerm always reads `account.json` by name, so a burst of short-lived sessions writing quota-less snapshots cannot hide the only useful reading. Install the status line on every host you want to monitor, including remote ones. Automatic macOS setup installs a JavaScript for Automation helper that uses the built-in `osascript`, so Python is not required. Automatic Windows setup installs `gputerm-claude-statusline.ps1` and registers an absolute, forward-slash path through `powershell.exe`, so it works whether Claude Code chooses Git Bash or PowerShell. Windows PowerShell 5.1 is built into supported Windows versions; Python is not required for the Windows helper.

Remote setup transfers the helper over SFTP rather than embedding it in a shell command, and every write goes through a temporary sibling that is unlinked-and-renamed into place (libssh2 pins SFTP to version 3, where a rename cannot overwrite an existing path). Embedding could not work reliably: macOS/BSD `base64` rejects a positional input file, and the Windows PowerShell form reached roughly 23,000 characters on the wire against cmd.exe's 8,191-character limit, which is what Windows OpenSSH uses for exec requests unless an administrator changed `DefaultShell`. Both the helper and `settings.json` are written through a temporary sibling and renamed into place, so a failed setup can never leave an empty helper where a working one was. A UTF-8 BOM in `settings.json`, which Windows editors and PowerShell redirection both write, is tolerated.

Only these fields are written: session id, working directory, model name and id, `context_window` (including `current_usage`), `cost.total_cost_usd`, `cost.total_duration_ms`, `rate_limits.{five_hour,seven_day}.{used_percentage,resets_at}`, a capture timestamp, and the agent pid when available. Prompts, responses, tool input and output, transcript paths, session names, and repository details are never copied. Snapshots older than seven days are pruned on the next run.

Two conditions come from Claude Code itself: `rate_limits` is present for subscription accounts only, and it appears after the session's first response. Rather than guessing a balance, the card names the step that is actually blocking — helper missing, helper present but empty, no status line configured, a different status line in the way, or installed and waiting for the first message.

## Architecture

```
┌───────────────────────────── Tauri window ─────────────────────────────┐
│  React 19 + TypeScript + Zustand + xterm.js                            │
│    invoke() ──────────────► Tauri commands (Rust)                      │
│    listen() ◄────────────── terminal-output · remote-telemetry ·       │
│                             llm-runtime-telemetry · sftp-progress ·    │
│                             terminal-closed                            │
├────────────────────────────────────────────────────────────────────────┤
│  Rust backend (ssh2 / libssh2)                                         │
│    • Terminal      – PTY shell, dedicated connection per terminal cell │
│    • Telemetry     – own connection, auto-reconnect with backoff       │
│    • SFTP ops      – pooled per-session "operations" connection        │
│    • Bulk transfer – dedicated connection per item, recursive/cancellable│
│    • LLM runtimes  – session-independent HTTP poller (ureq), one thread│
│                      per instance, read-only; optionally over an SSH   │
│                      direct-tcpip forward                              │
└────────────────────────────────────────────────────────────────────────┘
```

Long-running work is isolated: blocking SSH I/O runs on `spawn_blocking` threads so the UI never freezes, and each concern (shell / telemetry / transfers) fails independently.

**Data locations** (`%APPDATA%\GpuTerm` on Windows, `~/Library/Application Support/GpuTerm` on macOS, `~/.config/GpuTerm` on Linux):

| Location | Contents |
| --- | --- |
| `sessions.json` | Session profiles — host, port, username, key *path*, ProxyJump, and optional forwarding rules |
| `known_hosts.json` | Approved SHA-256 host key fingerprints |
| `app_settings.json` | UI preferences: the last local SFTP directory, terminal appearance, monitoring interval, display mode and ignored filesystems, and alert rules |
| `credentials.enc` | Versioned Argon2id parameters plus an AES-256-GCM encrypted and authenticated credential payload |
| `credential_index.json` | Non-secret session ids used only to show which profiles have a saved vault entry |
| `snippets.json` | Saved command names, text, return behavior, and optional profile ids. Never credentials |
| `llm_instances.json` | Registered LLM runtime instances — name, adapter, URL, poll interval, optional SSH tunnel profile id. Never the API key |
| `keys.json` | Key names, algorithms, public keys/fingerprints and profile assignments; private keys use separate vault entries |
| `db_instances.json` | DB names, engine/address/user and SSH profile; passwords use `db:<id>` vault entries |
| `vault_key.touchid` / login Keychain | Optional non-secret marker and OS-protected derived vault key, read after Touch ID verification |
| `vault_key.dpapi` | Optional Windows-account DPAPI-protected derived vault key, read after Windows Hello verification |

Normal vault storage encrypts private keys/passwords at rest. Explicit **unencrypted private-key export** writes plaintext by user choice; SQL monitoring creates the private temporary password file described above on the remote host. Key-file contents are not copied into profile JSON. Optional biometric unlock also persists a derived key under OS account protection.

## Development

```bash
npm run test                                    # frontend tests (Vitest)
npm run typecheck                               # TypeScript, no emit
npm run lint                                    # ESLint (React hooks rules)
cargo test --manifest-path src-tauri/Cargo.toml # backend tests
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings  # lints
npm run build                                   # TypeScript + Vite production build
```

`.github/workflows/ci.yml` runs all of the above on every push and pull request, plus a `cargo check` against the MSRV declared in `src-tauri/Cargo.toml`.

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
  agent_monitor.rs      Agent process trees + read-only session metadata
  macos_monitor.rs      macOS collectors (sysctl, vm_stat, ioreg)
  windows_monitor.rs    Windows collectors (PowerShell CIM, WDDM GPU counters)
  gpu_monitor.rs        GPU tool probing + vendor parsers
  resource_details.rs   On-demand CPU/RAM/GPU detail collection
  sftp.rs               Transfers, cancellation, SFTP commands
  session.rs            Connections, host keys, profiles, connection pool
```

</details>

### Releases & CI

Pushing a `v*` tag runs the [Release Build workflow](.github/workflows/release.yml), which creates the prerelease from [RELEASE_NOTES.md](./RELEASE_NOTES.md), builds the Windows `.exe` (NSIS), Debian `.deb`, and macOS `.dmg` on GitHub-hosted runners, and attaches them to the tag. The macOS job signs every nested code object inside-out, deep-signs and verifies the final app, builds the DMG from that verified bundle, mounts it, and verifies the enclosed app again. A `SHA256SUMS.txt` file is published after all installers succeed. The workflow can also be dispatched manually for an existing tag from the Actions page.

Each release also gets signed update bundles (the NSIS setup, the AppImage and a macOS `.app.tar.gz` packed from the ad-hoc signed app, each with a `.sig`) and a `latest.json` manifest built by [`scripts/make-updater-manifest.mjs`](scripts/make-updater-manifest.mjs). The manifest is copied to the fixed [`updater`](https://github.com/fortranmentis/GPUTERM/releases/tag/updater) release, which is the URL the app polls, because GitHub's “latest release” link skips pre-releases. Signing reads the `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` repository secrets; [`src-tauri/tauri.release.conf.json`](src-tauri/tauri.release.conf.json) turns on the update bundles only for release builds, so local `tauri build` runs need no key.

## FAQ

<details>
<summary><b>Does GpuTerm install anything on my servers?</b></summary>

Almost never, and never without you asking. Every metric comes from one-shot, read-only standard commands over SSH (`nvidia-smi`, `cat /proc/...`, `sysctl`, PowerShell `Get-CimInstance`, …). There are three exceptions, all narrow:

- **LLM runtime monitoring runs no remote command at all.** It issues HTTP GETs from your machine to the addresses you register. With an SSH tunnel it opens a forwarded channel, still without executing anything on the host.
- **Windows AI DASH may upload a collector script.** When a PowerShell command exceeds the `cmd.exe` command-line limit, GpuTerm writes a content-addressed `.ps1` under the connected user's `~/.gputerm/scripts` and runs it with `-File`. Those files contain only GpuTerm's collector code — no credentials, no collected output — and are pruned after seven days without use.
- **Claude Code quota monitoring installs a status-line script, but only when you press Set up.** It writes `gputerm-claude-statusline.*` into `~/.claude` on the monitored host so Claude can publish its own limits. Nothing else is copied, and no other metric depends on it.

</details>

<details>
<summary><b>Where are my passwords stored?</b></summary>

Passwords and key passphrases are stored in `credentials.enc`. Argon2id (64 MiB, 3 iterations) derives a 256-bit key and AES-256-GCM encrypts/authenticates the payload with a fresh nonce per write. The master password stays in memory; a derived key persists under OS protection only when biometric unlock is enabled. `sessions.json` has host metadata, key paths and managed public-key metadata, never secrets. Profile deletion removes its own local vault entries; server-side public keys require separate revocation.

Older Keychain/Credential Manager/Secret Service entries are not imported or accessed. After upgrading, enter each SSH password again. If you forget the master password, reset the vault; profiles remain, but saved passwords must be entered again and GpuTerm-managed keys are lost (export them beforehand, or set them up again). See the [data locations](#architecture) table.

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
| Pressing multiple keys shows `Terminal stream failed: transport read` | Re-download v1.1.3-beta: the refreshed installers remove the destructive SSH channel flush and serialize nonblocking input, PTY resize, and keepalive operations |
| Master password is rejected or forgotten | Check the password, or choose **Reset vault**. Profiles are kept, but all saved SSH passwords and GpuTerm-managed keys are deleted; enter the passwords again and set the keys up again |
| Host key mismatch | Verify the server fingerprint out-of-band, then remove the stale entry from `known_hosts.json` |
| GPU shows unavailable | Confirm a GPU tool is installed (`nvidia-smi`, `rocm-smi`, `amd-smi`, `xpu-smi`, or `intel_gpu_top`) or Linux `/sys/class/drm` is readable; other metrics still work regardless |
| Hybrid Windows PC only shows the dGPU | Update to the latest build. Idle Intel/AMD adapters are now retained from `Win32_VideoController` even when WDDM has not created a GPU Engine counter instance yet |
| AI DASH card is empty | Confirm `agy`, `codex`, or `claude` is running under a user visible to the telemetry account. CPU/RAM appears from the process tree; AGY token/context metadata additionally needs `python3` or `python` on the monitored host |
| Windows Claude setup succeeds but limits stay `n/a` | Run **Set up** again to replace the older `%USERPROFILE%` Python command, restart Claude Code, accept workspace trust, and send one message. Claude publishes subscription limits only after its first response |
| Local Windows session flashes console windows or has no monitoring data | Fixed in v1.1.6-beta — update the app; local PowerShell collectors now run without a console and use UTF-8 output |
| Windows remote shows “The system cannot find the path specified” | Fixed in v1.0.9-beta — older builds misdetected Windows hosts that have a `uname` port on PATH as Linux; update the app |
| Korean input splits into jamo in the terminal | Fixed for macOS/WebKit clients — update to the latest release |
| LLM instance shows `connection refused` at `127.0.0.1` | Check **Reached through** on its detail card. `Direct` means *this* machine's loopback; if the runtime is on another host, press **Edit** and set **Reach through** to that host's SSH profile |
| LLM instance shows `SSH host key not trusted` | Press **Test connection** on that instance once and accept the fingerprint, or open the same SSH session in a terminal. The background poller cannot show the prompt itself; monitoring resumes automatically afterwards |
| Temperatures are all `n/a` or *not available on this host* | Expected on WSL2, containers, and most VMs — they pass no `/sys/class/hwmon` through — and on macOS, where every temperature source needs root. Open the CPU/RAM/Disk detail view: the reason names the missing interface. Everything else keeps working |
| No RAM temperature on a bare-metal Linux box | Needs `modprobe jc42` (DDR3/DDR4 ECC) or `spd5118` (DDR5, kernel 6.10+), *and* a platform that exposes the SPD bus. Most non-ECC consumer boards never do, so there is nothing to load a driver for |
| No drive temperature on Linux | NVMe comes from the `nvme` driver's own hwmon sensor; SATA/SAS needs `modprobe drivetemp`. USB-bridged and RAID-backed disks usually pass no SMART through at all |
| Windows CPU temperature looks wrong | It is an ACPI thermal zone, not the CPU die — often a chassis or skin sensor. GpuTerm labels it as such and never colours it. Windows has no unprivileged way to read the die |
| vLLM metrics are all *not supported* | The server is probably running with `--disable-log-stats`, which publishes no `/metrics`. Restart it without that flag |

## Roadmap / Known limitations

- Biometric verification and Keychain/DPAPI access are application-gated, not hardware-bound. Real prompts and ad-hoc upgrade authorization need installed-device validation; Keychain may separately request access. Disabling the feature removes its stored unlock key and restores master-password-only operation.
- Containers depend on server tools, socket permissions and kubeconfig/RBAC; no local/macOS/Windows container collector or native RPM/Arch/Gentoo installer is added. Distribution parsing is fixture-tested, not validation of every distribution/version.
- SQL is a read-only statistics view. Missing/permission-restricted metrics stay unavailable; PostgreSQL activity/replication views need suitable privileges. Live runtime/cluster/DB checks remain environment-dependent. Password stdin is single-line and cleanup cannot cover an abruptly killed collector.
- Managed SSH key setup needs an unlocked vault, write access to the server's authorized-key file, and a server accepting the new key without a second factor. Verification never falls back to an agent or password. Key installation/revocation is a remote configuration change; rollback and old-key cleanup can fail on network or permission errors, so confirm server-side revocation when replacing/removing keys. Deleting a profile or resetting the vault alone does not revoke its server-side public key.
- `amd-smi` JSON support is covered by fixtures for several layouts; real AMD hardware validation remains pending. Missing fields stay unavailable, and fan speed/clock are not collected.
- Reply-based SSH liveness checks run after 15 seconds of inactivity and allow 8 seconds for a reply (roughly 23 seconds total). Servers that do not answer the initial probe fall back to keepalives/TCP detection; the detection time is therefore not a guarantee for every server. Live OTP and disconnect detection tests remain pending.
- Process termination rechecks the process start marker immediately before signalling and uses the current account's permissions. This reduces PID-reuse risk but does not make the check and signal atomic. OS notifications depend on installation and system notification settings; an in-app alert remains available when delivery fails.
- A host that asks for a one-time code at every sign-in can only be used by connections you start: background monitoring, SFTP, port forwards, and LLM tunnels open their own connections and cannot ask for a code, so they report that instead of retrying
- Usage history and alert state are kept in memory per session and reset when a session reconnects or the app restarts; alert thresholds are the same for every host
- Interrupted transfer resume is not implemented
- `known_hosts.json` uses SHA-256 fingerprints, not the OpenSSH known_hosts format
- Telemetry supports local and remote Linux, macOS (Apple Silicon included), and Windows hosts; Apple GPU power/temperature and per-core CPU usage need root `powermetrics` and are not shown
- GPU monitoring uses `nvidia-smi`, `rocm-smi`, `amd-smi`, `xpu-smi`, `intel_gpu_top`, Linux DRM/sysfs, macOS `ioreg`, or Windows WDDM performance counters; DRM shared-memory GPUs may expose utilization without dedicated VRAM, power, or temperature
- Agent CPU/RAM/process-tree totals are always available when the CLI process is visible to the monitoring user. AGY 1.0 token/context metadata needs Python 3 on the monitored host; AGY live quota/work state, Claude cost/subscriber limits, and provider-specific fields remain best effort because CLI log/status schemas vary. Unavailable fields show as n/a
- Windows remotes: requires Windows PowerShell 5.1+ (preinstalled); load averages don't exist and show as n/a; AMD/Intel GPUs report utilization and dedicated VRAM only (no power/temperature, needs Windows 10 1709+ with a WDDM 2.x driver); process owners and GPU process command lines need elevation and fall back to n/a / process names; `quser` is missing on Home editions, so the Users section stays empty there; hybrid iGPU+dGPU hosts show both cards (counters are attributed by adapter LUID from the DirectX registry, idle adapters remain visible without an activity counter, and a positional heuristic is used if the key is unavailable)
- Temperature availability differs sharply by platform. macOS shows none: `powermetrics` requires root and its `smc` sampler does not exist on Apple Silicon, and IOKit SMC sensors are not readable from a shell. Windows CPU temperature is an ACPI thermal zone — frequently a chassis sensor, unimplemented on many OEM firmwares, and never the die — so it is labelled as such and never colour-coded; Windows has no DIMM temperature interface at all. Linux DIMM labels are the SPD i2c address (`DIMM 0x18`), which does not identify the physical slot. `drivetemp` polls SMART and can keep spinning disks awake (`rmmod drivetemp` undoes it). Readings outside 1–150 °C are dropped as uninitialized-sensor artifacts, which also discards a genuine 0 °C
- LLM runtime monitoring is read-only and endpoint-based: models are not downloaded, deleted, started, or stopped, servers are not restarted, and no inference request is ever sent. Ollama therefore has no request, queue, TTFT, or tokens/s figures at all, and vLLM metrics depend on the version — anything the server does not publish is reported as *not supported* rather than as `0`
- LLM time series are kept in memory for one hour and are not written to disk, so they start empty after a restart
- An SSH-tunneled LLM instance needs its host key trusted first (via **Test connection** or by opening that session in a terminal once), because a background poller cannot show the fingerprint prompt. Its forwarded loopback port is reachable by any local process for as long as the app runs — the same exposure as a hand-run `ssh -L`. `https` cannot be tunneled, since the certificate would be validated against `127.0.0.1`
- macOS installer currently targets Apple Silicon only (Intel Macs: build from source)

Issues and pull requests are welcome — please run the test suites above before submitting.

## License

[PolyForm Noncommercial 1.0.0](./LICENSE) © GpuTerm contributors. **Free for personal and noncommercial use; commercial use is not permitted** — see the [license](./LICENSE) or contact the maintainer for a commercial license. Built with [Tauri](https://tauri.app), [React](https://react.dev), [xterm.js](https://xtermjs.org), and [ssh2](https://crates.io/crates/ssh2); those third-party components remain under their own (open-source) licenses.
