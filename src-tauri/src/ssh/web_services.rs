//! Web UIs running on a connected host (TensorBoard, Jupyter, MLflow, Ray,
//! Streamlit, Gradio, code-server and other listening ports), and opening one
//! in the local browser through a temporary SSH forward.
//!
//! Detection lists the listening TCP sockets with their owning processes
//! (`ss` or `/proc/net/tcp` on Linux, `lsof` on macOS, `Get-NetTCPConnection`
//! on Windows), the user's processes, Jupyter's runtime files (which carry
//! the port, base URL and login token) and, on Linux, the ports Docker
//! containers expose. Only processes owned by the login user are considered,
//! so on a shared server another user's notebook never shows up.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::State;

use crate::ssh::forwarding::{start_one_forward, ForwardKind, ForwardSpec};
use crate::ssh::resource_details::resolve_remote_os;
use crate::ssh::session::{target_for_active_session, with_ops_session, AppState};
use crate::ssh::system_monitor::{
    run_remote_command_for, run_remote_command_with_budget, RemoteOs,
};

const SCAN_TIMEOUT_MS: u32 = 8_000;
const SCAN_BUDGET_SECS: u64 = 5;
/// PowerShell's startup alone can take a few seconds on a cold host.
const WINDOWS_SCAN_TIMEOUT_MS: u32 = 15_000;
/// How long opening waits for a new forward to start listening.
const FORWARD_READY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PATH_LEN: usize = 4096;
/// Gradio picks the first free port from 7860 up.
const GRADIO_PORTS: std::ops::RangeInclusive<u16> = 7860..=7870;

/// Every row starts with a tag, so a stray line from a login script is ignored.
/// The runtime path is split so that this script's own command line never
/// classifies as a Jupyter server.
const LINUX_SCAN: &str = r#"if command -v ss >/dev/null 2>&1; then ss -tlnp 2>/dev/null | sed 's/^/L /'; else awk '$4=="0A"{print "N "$2}' /proc/net/tcp /proc/net/tcp6 2>/dev/null; fi
ps -u "$(id -u)" -o pid=,args= 2>/dev/null | cut -c1-400 | sed 's/^/P /'
d="${JUPYTER_RUNTIME_DIR:-$HOME/.local/share/jupy""ter/runtime}"
for f in "$d"/jpserver-*.json "$d"/nbserver-*.json; do [ -f "$f" ] && printf 'J %s\n' "$(tr -d '\n' < "$f")"; done
if command -v docker >/dev/null 2>&1; then docker ps -q 2>/dev/null | head -n 50 | xargs -r docker inspect --format '{{.Name}}|{{.Config.Image}}|{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}|{{range $p, $v := .Config.ExposedPorts}}{{$p}} {{end}}' 2>/dev/null | sed 's/^/D /'; fi
true"#;

const MAC_SCAN: &str = r#"lsof -nP -a -u "$(id -u)" -iTCP -sTCP:LISTEN -F pn 2>/dev/null | awk '/^p/{p=substr($0,2)} /^n/{print "M "p" "substr($0,2)}'
ps -u "$(id -u)" -o pid=,args= 2>/dev/null | cut -c1-400 | sed 's/^/P /'
d="${JUPYTER_RUNTIME_DIR:-$HOME/Library/Jupy""ter/runtime}"
for f in "$d"/jpserver-*.json "$d"/nbserver-*.json; do [ -f "$f" ] && printf 'J %s\n' "$(tr -d '\n' < "$f")"; done
true"#;

/// Without elevation Windows hides other users' command lines, which keeps
/// their servers out of the list just as `ps -u` does elsewhere.
const WINDOWS_SCAN: &str = r#"$ErrorActionPreference = 'SilentlyContinue'
$listen = @(Get-NetTCPConnection -State Listen)
foreach ($c in $listen) { "M $($c.OwningProcess) $($c.LocalAddress):$($c.LocalPort)" }
$ids = @($listen | ForEach-Object { [int]$_.OwningProcess } | Sort-Object -Unique)
foreach ($p in Get-CimInstance Win32_Process) {
  if ($ids -contains [int]$p.ProcessId -and $p.CommandLine) {
    $line = $p.CommandLine -replace "[`r`n]", ' '
    if ($line.Length -gt 400) { $line = $line.Substring(0, 400) }
    "P $($p.ProcessId) $line"
  }
}
$d = if ($env:JUPYTER_RUNTIME_DIR) { $env:JUPYTER_RUNTIME_DIR } else { Join-Path $env:APPDATA ('jupy' + 'ter\runtime') }
foreach ($f in @(Get-ChildItem -Path $d -Filter '*server-*.json')) { 'J ' + ((Get-Content -Raw $f.FullName) -replace "[`r`n]", '') }"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WebServiceKind {
    Tensorboard,
    Jupyter,
    Mlflow,
    Ray,
    Streamlit,
    Gradio,
    CodeServer,
    /// A listening port of the user's that is not one of the above.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebService {
    pub kind: WebServiceKind,
    pub pid: Option<u32>,
    /// Where the server connects for the forward: "localhost" for loopback
    /// and wildcard listeners, a container's address for container ports.
    pub host: String,
    pub port: u16,
    /// Path and query to open, including Jupyter's login token.
    pub path: String,
    /// TensorBoard's logdir, Jupyter's root directory, or the program.
    pub detail: Option<String>,
    /// The Docker container serving it, if any.
    pub container: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebServiceScan {
    pub supported: bool,
    pub services: Vec<WebService>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Listener {
    host: String,
    port: u16,
    pids: Vec<u32>,
}

#[derive(Debug, Clone, Default)]
struct JupyterRuntime {
    pid: Option<u32>,
    port: Option<u16>,
    base_url: String,
    token: String,
    root_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Classified {
    Known(WebServiceKind),
    /// Not a known web UI; its ports may still be listed as "other".
    Unknown,
    /// Never listed: kernels, editor backends, agents.
    Ignored,
}

/// The host to forward to for a listening address.
fn forward_host(address: &str) -> String {
    let address = address.split('%').next().unwrap_or(address);
    let bare = address.trim_matches(['[', ']']);
    if bare.is_empty() || bare == "*" || bare.eq_ignore_ascii_case("localhost") {
        return "localhost".to_string();
    }
    match bare.parse::<IpAddr>() {
        Ok(ip) if ip.is_loopback() || ip.is_unspecified() => "localhost".to_string(),
        Ok(ip) => ip.to_string(),
        Err(_) => bare.to_string(),
    }
}

/// `127.0.0.1:6006`, `[::1]:8888`, `*:6006`, `0.0.0.0%lo:53`, `:::8888`.
fn parse_endpoint(endpoint: &str) -> Option<(String, u16)> {
    let (host, port) = endpoint.rsplit_once(':')?;
    let port = port.parse::<u16>().ok().filter(|port| *port > 0)?;
    Some((forward_host(host), port))
}

/// One `/proc/net/tcp{,6}` local address, e.g. `0100007F:1F90`.
fn parse_proc_endpoint(hex: &str) -> Option<(String, u16)> {
    let (address, port) = hex.split_once(':')?;
    let port = u16::from_str_radix(port, 16)
        .ok()
        .filter(|port| *port > 0)?;
    let host = if address.len() == 8 {
        let raw = u32::from_str_radix(address, 16).ok()?;
        forward_host(&Ipv4Addr::from(raw.swap_bytes()).to_string())
    } else if address.chars().all(|c| c == '0') || address == "00000000000000000000000001000000" {
        // `::` or `::1`.
        "localhost".to_string()
    } else {
        // Another IPv6 address; connecting to it by name is not worth the
        // byte-order decoding for a case this rare.
        return None;
    };
    Some((host, port))
}

fn pids_in(process: &str) -> Vec<u32> {
    process
        .split("pid=")
        .skip(1)
        .filter_map(|rest| {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        })
        .collect()
}

fn classify(args: &str) -> Classified {
    let lower = args.to_ascii_lowercase();
    const IGNORED: [&str; 7] = [
        "ipykernel",
        ".vscode-server",
        ".cursor-server",
        "vscode-remote",
        "language_server",
        "ssh-agent",
        "gpg-agent",
    ];
    if IGNORED.iter().any(|needle| lower.contains(needle)) {
        return Classified::Ignored;
    }
    const JUPYTER: [&str; 10] = [
        "jupyter-lab",
        "jupyter-notebook",
        "jupyter-server",
        "jupyter lab",
        "jupyter notebook",
        "jupyter server",
        "-m jupyterlab",
        "-m notebook",
        "-m jupyter_server",
        "jupyterhub-singleuser",
    ];
    let kind = if lower.contains("tensorboard") {
        WebServiceKind::Tensorboard
    } else if JUPYTER.iter().any(|needle| lower.contains(needle)) {
        WebServiceKind::Jupyter
    } else if lower.contains("mlflow") {
        WebServiceKind::Mlflow
    } else if lower.contains("ray/dashboard") || lower.contains("ray\\dashboard") {
        WebServiceKind::Ray
    } else if lower.contains("streamlit") {
        WebServiceKind::Streamlit
    } else if lower.contains("code-server") {
        WebServiceKind::CodeServer
    } else if lower.contains("gradio") {
        WebServiceKind::Gradio
    } else {
        return Classified::Unknown;
    };
    Classified::Known(kind)
}

/// The value of `--flag value` or `--flag=value`.
fn flag_value(args: &str, flag: &str) -> Option<String> {
    let mut words = args.split_whitespace();
    while let Some(word) = words.next() {
        if word == flag {
            return words.next().map(str::to_string);
        }
        if let Some(value) = word
            .strip_prefix(flag)
            .and_then(|rest| rest.strip_prefix('='))
        {
            return Some(value.to_string());
        }
    }
    None
}

fn port_flag(args: &str, flag: &str) -> Option<u16> {
    flag_value(args, flag).and_then(|value| value.rsplit(':').next()?.parse().ok())
}

/// The port a known server uses when `ss` cannot say which socket is its own.
fn configured_port(kind: WebServiceKind, args: &str) -> Option<u16> {
    match kind {
        WebServiceKind::Tensorboard => Some(port_flag(args, "--port").unwrap_or(6006)),
        WebServiceKind::Mlflow => Some(
            port_flag(args, "--port")
                .or_else(|| port_flag(args, "-p"))
                .unwrap_or(5000),
        ),
        WebServiceKind::Ray => Some(port_flag(args, "--port").unwrap_or(8265)),
        WebServiceKind::Streamlit => Some(port_flag(args, "--server.port").unwrap_or(8501)),
        WebServiceKind::CodeServer => Some(port_flag(args, "--bind-addr").unwrap_or(8080)),
        WebServiceKind::Gradio => Some(7860),
        WebServiceKind::Jupyter | WebServiceKind::Other => None,
    }
}

/// `python3 app.py` → `app.py`, `python -m mymodule` → `mymodule`,
/// `/usr/bin/node server.js` → `node server.js`.
fn program_label(args: &str) -> String {
    let mut words = args.split_whitespace();
    let base = |word: &str| {
        word.rsplit(['/', '\\'])
            .next()
            .unwrap_or(word)
            .trim_end_matches(".exe")
            .to_string()
    };
    let Some(first) = words.next().map(base) else {
        return String::new();
    };
    if first.to_ascii_lowercase().starts_with("python") {
        match words.next() {
            Some("-m") => return words.next().unwrap_or("python").to_string(),
            Some(script) if !script.starts_with('-') => return base(script),
            _ => {}
        }
    }
    match words.next() {
        Some(arg) if !arg.starts_with('-') && first == "node" => format!("{first} {}", base(arg)),
        _ => first,
    }
}

fn parse_runtime(json: &str) -> Option<JupyterRuntime> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_string);
    Some(JupyterRuntime {
        pid: value
            .get("pid")
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok()),
        port: value
            .get("port")
            .and_then(|v| v.as_u64())
            .and_then(|v| u16::try_from(v).ok()),
        base_url: text("base_url").unwrap_or_else(|| "/".to_string()),
        token: text("token").unwrap_or_default(),
        root_dir: text("root_dir").or_else(|| text("notebook_dir")),
    })
}

fn jupyter_path(runtime: Option<&JupyterRuntime>) -> String {
    let Some(runtime) = runtime else {
        return "/".to_string();
    };
    let mut path = runtime.base_url.clone();
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    if !path.ends_with('/') {
        path.push('/');
    }
    let token_ok = !runtime.token.is_empty()
        && runtime
            .token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if token_ok {
        path.push_str("?token=");
        path.push_str(&runtime.token);
    }
    path
}

/// What a container port probably serves, from its image name or the port.
fn container_kind(image: &str, port: u16) -> WebServiceKind {
    let image = image.to_ascii_lowercase();
    match () {
        _ if image.contains("tensorboard") || port == 6006 => WebServiceKind::Tensorboard,
        _ if image.contains("jupyter") || image.contains("notebook") || port == 8888 => {
            WebServiceKind::Jupyter
        }
        _ if image.contains("mlflow") || port == 5000 => WebServiceKind::Mlflow,
        _ if image.contains("streamlit") || port == 8501 => WebServiceKind::Streamlit,
        _ if image.contains("ray") || port == 8265 => WebServiceKind::Ray,
        _ if image.contains("code-server") => WebServiceKind::CodeServer,
        _ if GRADIO_PORTS.contains(&port) => WebServiceKind::Gradio,
        _ => WebServiceKind::Other,
    }
}

/// A `D` row: `/name|image|172.17.0.2 |8888/tcp 6006/tcp `.
fn parse_container(row: &str) -> Vec<WebService> {
    let fields: Vec<&str> = row.split('|').collect();
    let [name, image, addresses, ports] = fields[..] else {
        return Vec::new();
    };
    // Containers on the host network show up as ordinary processes.
    let Some(address) = addresses
        .split_whitespace()
        .find(|address| address.parse::<IpAddr>().is_ok())
    else {
        return Vec::new();
    };
    let name = name.trim().trim_start_matches('/').to_string();
    ports
        .split_whitespace()
        .filter_map(|port| port.strip_suffix("/tcp")?.parse::<u16>().ok())
        .filter(|port| *port > 0)
        .map(|port| WebService {
            kind: container_kind(image, port),
            pid: None,
            host: address.to_string(),
            port,
            path: "/".to_string(),
            detail: Some(image.trim().to_string()).filter(|image| !image.is_empty()),
            container: Some(name.clone()),
        })
        .collect()
}

fn parse_scan(output: &str) -> Vec<WebService> {
    let mut listeners: Vec<Listener> = Vec::new();
    let mut processes: Vec<(u32, String)> = Vec::new();
    let mut runtimes: Vec<JupyterRuntime> = Vec::new();
    let mut containers: Vec<WebService> = Vec::new();

    for line in output.lines() {
        let Some((tag, rest)) = line.split_once(' ') else {
            continue;
        };
        match tag {
            "L" => {
                let fields: Vec<&str> = rest.split_whitespace().collect();
                if fields.len() < 5 {
                    continue;
                }
                if let Some((host, port)) = parse_endpoint(fields[3]) {
                    let pids = pids_in(&fields[5..].join(" "));
                    listeners.push(Listener { host, port, pids });
                }
            }
            "M" => {
                let mut fields = rest.split_whitespace();
                let pid = fields.next().and_then(|pid| pid.parse::<u32>().ok());
                if let (Some(pid), Some((host, port))) =
                    (pid, fields.next().and_then(parse_endpoint))
                {
                    listeners.push(Listener {
                        host,
                        port,
                        pids: vec![pid],
                    });
                }
            }
            "N" => {
                if let Some((host, port)) = parse_proc_endpoint(rest.trim()) {
                    listeners.push(Listener {
                        host,
                        port,
                        pids: Vec::new(),
                    });
                }
            }
            "P" => {
                let rest = rest.trim_start();
                if let Some((pid, args)) = rest.split_once(char::is_whitespace) {
                    if let Ok(pid) = pid.parse() {
                        processes.push((pid, args.trim().to_string()));
                    }
                }
            }
            "J" => runtimes.extend(parse_runtime(rest)),
            "D" => containers.extend(parse_container(rest)),
            _ => {}
        }
    }

    let listener_on = |port: u16| listeners.iter().find(|listener| listener.port == port);
    let mut services: Vec<WebService> = Vec::new();
    let mut seen: HashSet<(String, u16)> = HashSet::new();
    let mut add = |service: WebService| {
        if seen.insert((service.host.clone(), service.port)) {
            services.push(service);
        }
    };

    // Known servers first, so a port is never claimed as "other" before the
    // server that owns it.
    let mut others: Vec<WebService> = Vec::new();
    for (pid, args) in &processes {
        let classified = classify(args);
        if classified == Classified::Ignored {
            continue;
        }
        let runtime = runtimes.iter().find(|runtime| runtime.pid == Some(*pid));
        let owned: Vec<(String, u16)> = listeners
            .iter()
            .filter(|listener| listener.pids.contains(pid))
            .map(|listener| (listener.host.clone(), listener.port))
            .collect();
        let Classified::Known(kind) = classified else {
            // An unknown program: only its own sockets, and only unprivileged
            // ports. A Python server on Gradio's ports is most likely Gradio.
            for (host, port) in owned.into_iter().filter(|(_, port)| *port >= 1024) {
                let gradio = GRADIO_PORTS.contains(&port) && program_label(args).ends_with(".py");
                others.push(WebService {
                    kind: if gradio {
                        WebServiceKind::Gradio
                    } else {
                        WebServiceKind::Other
                    },
                    pid: Some(*pid),
                    host,
                    port,
                    path: "/".to_string(),
                    detail: Some(program_label(args)).filter(|label| !label.is_empty()),
                    container: None,
                });
            }
            continue;
        };
        // Without socket owners (no `ss`, or it hid them), fall back to the
        // port the process says it uses, if something listens there.
        let ports: Vec<(String, u16)> = if !owned.is_empty() {
            owned
        } else {
            let port = match kind {
                WebServiceKind::Jupyter => runtime.and_then(|runtime| runtime.port),
                _ => configured_port(kind, args),
            };
            port.and_then(listener_on)
                .map(|listener| vec![(listener.host.clone(), listener.port)])
                .unwrap_or_default()
        };
        for (host, port) in ports {
            let (path, detail) = match kind {
                WebServiceKind::Tensorboard => ("/".to_string(), flag_value(args, "--logdir")),
                WebServiceKind::Jupyter => (
                    jupyter_path(runtime),
                    runtime.and_then(|runtime| runtime.root_dir.clone()),
                ),
                // Backend/artifact URIs may embed DB or storage credentials.
                // Never turn these command-line options into UI/MCP metadata.
                WebServiceKind::Mlflow => ("/".to_string(), None),
                WebServiceKind::Streamlit => ("/".to_string(), streamlit_script(args)),
                _ => ("/".to_string(), None),
            };
            add(WebService {
                kind,
                pid: Some(*pid),
                host,
                port,
                path,
                detail,
                container: None,
            });
        }
    }

    // A Jupyter server whose process `ps` did not report, still listening.
    for runtime in &runtimes {
        let Some(listener) = runtime.port.and_then(listener_on) else {
            continue;
        };
        add(WebService {
            kind: WebServiceKind::Jupyter,
            pid: runtime.pid,
            host: listener.host.clone(),
            port: listener.port,
            path: jupyter_path(Some(runtime)),
            detail: runtime.root_dir.clone(),
            container: None,
        });
    }
    for service in containers.into_iter().chain(others) {
        add(service);
    }

    services.sort_by_key(|service| (service.kind, service.port));
    services
}

/// `streamlit run app.py` → `app.py`.
fn streamlit_script(args: &str) -> Option<String> {
    let mut words = args.split_whitespace().skip_while(|word| *word != "run");
    words.next()?;
    words.next().map(str::to_string)
}

fn session_is_local(state: &AppState, session_id: &str) -> Result<bool, String> {
    state
        .active_connections
        .lock()
        .map_err(|_| "Active session state is unavailable".to_string())?
        .get(session_id)
        .map(|connection| connection.profile.is_local)
        .ok_or_else(|| "No active terminal session is available".to_string())
}

#[tauri::command]
pub async fn scan_web_services(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<WebServiceScan, String> {
    if session_is_local(&state, &session_id)? {
        return Ok(WebServiceScan {
            supported: false,
            services: Vec::new(),
        });
    }
    let target = target_for_active_session(&state, &session_id)?;
    let ops = Arc::clone(&state.ops_sessions);
    let os_cache = Arc::clone(&state.remote_os_cache);
    tauri::async_runtime::spawn_blocking(move || {
        with_ops_session(&ops, &target, SCAN_TIMEOUT_MS, |session| {
            let output = match resolve_remote_os(&os_cache, &target.session_id, session) {
                RemoteOs::Linux => {
                    run_remote_command_with_budget(session, LINUX_SCAN, SCAN_BUDGET_SECS)?
                }
                RemoteOs::MacOs => {
                    run_remote_command_with_budget(session, MAC_SCAN, SCAN_BUDGET_SECS)?
                }
                RemoteOs::Windows => {
                    session.set_timeout(WINDOWS_SCAN_TIMEOUT_MS);
                    run_remote_command_for(session, RemoteOs::Windows, WINDOWS_SCAN)?
                }
            };
            Ok(WebServiceScan {
                supported: true,
                services: parse_scan(&output),
            })
        })
    })
    .await
    .map_err(|error| format!("Web service scan failed: {}", error))?
}

/// A host the server may connect to: an IP literal or a DNS name.
fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && (host.parse::<IpAddr>().is_ok()
            || host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'))
}

/// Path and query of the page to open. Printable ASCII only, so nothing can
/// smuggle a second argument or a different scheme to the URL handler.
fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= MAX_PATH_LEN
        && path
            .chars()
            .all(|c| c.is_ascii_graphic() && !matches!(c, '"' | '<' | '>' | '\\' | '`'))
}

/// The remote port when it is free here too, so the address is familiar;
/// otherwise any free port.
fn pick_local_port(preferred: u16) -> Result<u16, String> {
    if preferred >= 1024 && TcpListener::bind((Ipv4Addr::LOCALHOST, preferred)).is_ok() {
        return Ok(preferred);
    }
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|error| format!("No free local port: {error}"))
}

fn local_url(port: u16, path: &str) -> String {
    format!("http://127.0.0.1:{port}{path}")
}

/// Opens a web server on the session's host in the default browser. On a
/// remote host it forwards a local port first, reusing the forward from an
/// earlier open. Returns the URL that was opened.
#[tauri::command(async)]
pub fn open_web_service(
    state: State<'_, AppState>,
    session_id: String,
    host: String,
    port: u16,
    path: String,
) -> Result<String, String> {
    if port == 0 {
        return Err("Invalid port".to_string());
    }
    let path = if path.is_empty() {
        "/".to_string()
    } else {
        path
    };
    if !valid_path(&path) {
        return Err("This link cannot be opened".to_string());
    }
    let host = forward_host(host.trim());
    if !valid_host(&host) {
        return Err(format!("Invalid host: {host}"));
    }

    if session_is_local(&state, &session_id)? {
        let url = local_url(port, &path);
        crate::browser::open_in_browser(&url)?;
        return Ok(url);
    }

    let target = target_for_active_session(&state, &session_id)?;
    let manager = &state.port_forwards;
    let forward_id = format!("web:{host}:{port}");
    let live_port = |statuses: &[crate::ssh::forwarding::ForwardStatus]| {
        statuses
            .iter()
            .find(|status| status.spec.id == forward_id && status.state == "listening")
            .map(|status| status.bound_port.unwrap_or(status.spec.bind_port))
    };

    let local_port = match live_port(&manager.statuses(Some(&session_id))) {
        Some(port) => port,
        None => {
            let spec = ForwardSpec {
                id: forward_id.clone(),
                kind: ForwardKind::Local,
                bind_host: "127.0.0.1".to_string(),
                bind_port: pick_local_port(port)?,
                dest_host: Some(host.clone()),
                dest_port: Some(port),
                enabled: true,
            };
            start_one_forward(manager, &target, spec)?;
            let deadline = Instant::now() + FORWARD_READY_TIMEOUT;
            loop {
                let statuses = manager.statuses(Some(&session_id));
                if let Some(port) = live_port(&statuses) {
                    break port;
                }
                let failed = statuses
                    .iter()
                    .find(|status| status.spec.id == forward_id && status.state == "error");
                if let Some(status) = failed {
                    return Err(status
                        .message
                        .clone()
                        .unwrap_or_else(|| "The port forward failed".to_string()));
                }
                if Instant::now() >= deadline {
                    return Err("The port forward did not start in time".to_string());
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    };

    let url = local_url(local_port, &path);
    crate::browser::open_in_browser(&url)?;
    Ok(url)
}

/// Opens an http(s) link from the terminal in the default browser.
#[tauri::command]
pub fn open_link(url: String) -> Result<(), String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or("");
    let ok = !rest.is_empty()
        && url.len() <= MAX_PATH_LEN
        && url
            .chars()
            .all(|c| c.is_ascii_graphic() && !matches!(c, '"' | '<' | '>' | '\\' | '`'));
    if !ok {
        return Err("Only http and https links can be opened".to_string());
    }
    crate::browser::open_in_browser(&url)
}

/// A page path without Jupyter's login token, for anything that leaves the
/// app (the MCP tools, logs).
pub(crate) fn without_token(path: &str) -> String {
    let Some((base, query)) = path.split_once('?') else {
        return path.to_string();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| !pair.is_empty() && !pair.starts_with("token="))
        .collect();
    if kept.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", kept.join("&"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SS_OUTPUT: &str = "\
L State  Recv-Q Send-Q Local Address:Port Peer Address:PortProcess
L LISTEN 0      128          0.0.0.0:22        0.0.0.0:*
L LISTEN 0      4096       127.0.0.1:6006      0.0.0.0:*    users:((\"tensorboard\",pid=4100,fd=12))
L LISTEN 0      4096           [::1]:6006         [::]:*    users:((\"tensorboard\",pid=4100,fd=13))
L LISTEN 0      128          0.0.0.0:8888      0.0.0.0:*    users:((\"jupyter-lab\",pid=5200,fd=7))
L LISTEN 0      128        127.0.0.1:41003     0.0.0.0:*    users:((\"python\",pid=5300,fd=20))
L LISTEN 0      128        127.0.0.1:7861      0.0.0.0:*    users:((\"python\",pid=5400,fd=9))
L LISTEN 0      128          0.0.0.0:9000      0.0.0.0:*    users:((\"node\",pid=5500,fd=21))
L LISTEN 0      128        127.0.0.1:39811     0.0.0.0:*    users:((\"node\",pid=5600,fd=18))
L LISTEN 0      128          0.0.0.0:5000      0.0.0.0:*    users:((\"gunicorn\",pid=5700,fd=5))
P    4100 /usr/bin/python3 /home/me/.local/bin/tensorboard --logdir runs/exp42 --port 6006
P    5200 /home/me/venv/bin/python /home/me/venv/bin/jupyter-lab --no-browser
P    5300 /home/me/venv/bin/python -m ipykernel_launcher -f /home/me/.local/share/jupyter/runtime/kernel-1.json
P    5400 python3 demo_app.py
P    5500 /usr/bin/node server.js --port 9000
P    5600 /home/me/.vscode-server/bin/abc/node /home/me/.vscode-server/bin/abc/out/server-main.js
P    5700 /home/me/venv/bin/python /home/me/venv/bin/gunicorn -b 0.0.0.0:5000 mlflow.server:app
J {  \"base_url\": \"/\",  \"pid\": 5200,  \"port\": 8888,  \"root_dir\": \"/home/me/projects\",  \"token\": \"abc123\"}
D /notebook|jupyter/scipy-notebook:latest|172.17.0.2 |8888/tcp
D /db|postgres:16|172.17.0.3 |5432/tcp
D /hostnet|nginx||80/tcp
";

    fn find(services: &[WebService], port: u16, host: &str) -> WebService {
        services
            .iter()
            .find(|service| service.port == port && service.host == host)
            .cloned()
            .unwrap_or_else(|| panic!("no service on {host}:{port} in {services:?}"))
    }

    #[test]
    fn finds_known_servers_from_ss() {
        let services = parse_scan(SS_OUTPUT);
        assert_eq!(
            find(&services, 6006, "localhost"),
            WebService {
                kind: WebServiceKind::Tensorboard,
                pid: Some(4100),
                host: "localhost".into(),
                port: 6006,
                path: "/".into(),
                detail: Some("runs/exp42".into()),
                container: None,
            }
        );
        let jupyter = find(&services, 8888, "localhost");
        assert_eq!(jupyter.kind, WebServiceKind::Jupyter);
        assert_eq!(jupyter.path, "/?token=abc123");
        assert_eq!(jupyter.detail.as_deref(), Some("/home/me/projects"));
        assert_eq!(
            find(&services, 5000, "localhost").kind,
            WebServiceKind::Mlflow
        );
    }

    #[test]
    fn lists_other_ports_and_guesses_gradio() {
        let services = parse_scan(SS_OUTPUT);
        let gradio = find(&services, 7861, "localhost");
        assert_eq!(gradio.kind, WebServiceKind::Gradio);
        assert_eq!(gradio.detail.as_deref(), Some("demo_app.py"));
        let other = find(&services, 9000, "localhost");
        assert_eq!(other.kind, WebServiceKind::Other);
        assert_eq!(other.detail.as_deref(), Some("node server.js"));
        // Kernels, editor backends and root's sshd never show up.
        assert!(!services
            .iter()
            .any(|service| [41003, 39811, 22].contains(&service.port)));
    }

    #[test]
    fn mlflow_connection_strings_never_become_service_metadata() {
        for args in [
            "mlflow server --backend-store-uri postgresql://user:db-secret@db/private",
            "mlflow server --default-artifact-root https://user:storage-secret@store/private?token=secret",
        ] {
            let output = format!(
                "L LISTEN 0 128 127.0.0.1:5000 0.0.0.0:* users:((\"mlflow\",pid=42,fd=3))\nP 42 {args}\n"
            );
            let services = parse_scan(&output);
            let service = find(&services, 5000, "localhost");
            assert_eq!(service.kind, WebServiceKind::Mlflow);
            assert_eq!(service.detail, None);
            let metadata = serde_json::to_string(&services).unwrap();
            for sensitive in ["db-secret", "storage-secret", "private", "token="] {
                assert!(!metadata.contains(sensitive), "leaked {sensitive}");
            }
        }
    }

    #[test]
    fn lists_container_ports_by_address() {
        let services = parse_scan(SS_OUTPUT);
        let notebook = find(&services, 8888, "172.17.0.2");
        assert_eq!(notebook.kind, WebServiceKind::Jupyter);
        assert_eq!(notebook.container.as_deref(), Some("notebook"));
        assert_eq!(notebook.path, "/");
        assert_eq!(
            find(&services, 5432, "172.17.0.3").kind,
            WebServiceKind::Other
        );
        assert!(!services.iter().any(|service| service.port == 80));
    }

    #[test]
    fn falls_back_to_proc_net_tcp_and_flags() {
        let output = "\
N 0100007F:1776
N 00000000:1F90
N 00000000:20F5
P 900 python -m tensorboard.main --logdir=/data/logs
P 901 jupyter notebook --port=8080
P 902 streamlit run dashboard.py --server.port 8437
J {\"pid\": 901, \"port\": 8080, \"base_url\": \"/user/me\", \"token\": \"\"}
";
        let services = parse_scan(output);
        assert_eq!(services.len(), 3);
        assert_eq!(services[0].port, 6006);
        assert_eq!(services[0].detail.as_deref(), Some("/data/logs"));
        assert_eq!(services[1].kind, WebServiceKind::Jupyter);
        assert_eq!(services[1].path, "/user/me/");
        assert_eq!(services[2].kind, WebServiceKind::Streamlit);
        assert_eq!(services[2].port, 8437);
        assert_eq!(services[2].detail.as_deref(), Some("dashboard.py"));
    }

    #[test]
    fn reads_lsof_and_windows_rows() {
        let mac = "\
M 300 127.0.0.1:6006
M 300 [::1]:6006
M 301 *:8501
P 300 /opt/homebrew/bin/python3 /Users/me/.venv/bin/tensorboard --logdir logs
P 301 /Users/me/.venv/bin/python -m streamlit run app.py
";
        let services = parse_scan(mac);
        assert_eq!(services.len(), 2);
        assert_eq!(services[0].kind, WebServiceKind::Tensorboard);
        assert_eq!(services[1].kind, WebServiceKind::Streamlit);

        let windows = "\
M 4412 ::1:8888
M 4412 127.0.0.1:8888
M 4 0.0.0.0:445
P 4412 \"C:\\Users\\me\\venv\\Scripts\\python.exe\" \"C:\\Users\\me\\venv\\Scripts\\jupyter-lab.exe\"
J {\"pid\": 4412, \"port\": 8888, \"base_url\": \"/\", \"token\": \"t0k\"}
";
        let services = parse_scan(windows);
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].host, "localhost");
        assert_eq!(services[0].path, "/?token=t0k");
    }

    #[test]
    fn ignores_processes_that_do_not_listen_and_kernels() {
        let output = "\
P 10 tensorboard --logdir runs
P 11 python -m ipykernel_launcher
";
        assert!(parse_scan(output).is_empty());
    }

    #[test]
    fn unknown_tags_and_garbage_are_ignored() {
        assert!(parse_scan("Welcome to Ubuntu\nJ {not json\nL x\nD a|b\nM x y\n").is_empty());
    }

    #[test]
    fn endpoints_and_labels() {
        assert_eq!(
            parse_endpoint("10.0.0.5:6006"),
            Some(("10.0.0.5".into(), 6006))
        );
        assert_eq!(parse_endpoint("*:6006"), Some(("localhost".into(), 6006)));
        assert_eq!(
            parse_endpoint("[::]:8888"),
            Some(("localhost".into(), 8888))
        );
        assert_eq!(parse_endpoint(":::8888"), Some(("localhost".into(), 8888)));
        assert_eq!(
            parse_endpoint("0.0.0.0%lo:53"),
            Some(("localhost".into(), 53))
        );
        assert_eq!(parse_endpoint("127.0.0.1:0"), None);
        assert_eq!(program_label("python -m http.server 8000"), "http.server");
        assert_eq!(
            program_label("/Library/Frameworks/Python.framework/Resources/Python.app/Contents/MacOS/Python -m http.server"),
            "http.server"
        );
        assert_eq!(program_label("C:\\Tools\\app.exe --serve"), "app");
        assert_eq!(
            configured_port(
                WebServiceKind::CodeServer,
                "code-server --bind-addr 127.0.0.1:8443"
            ),
            Some(8443)
        );
    }

    #[test]
    fn tokens_with_unexpected_characters_are_dropped() {
        let runtime = JupyterRuntime {
            base_url: "/".into(),
            token: "a&b".into(),
            ..Default::default()
        };
        assert_eq!(jupyter_path(Some(&runtime)), "/");
    }

    #[test]
    fn paths_and_hosts_are_validated() {
        assert!(valid_path("/lab?token=abc"));
        assert!(!valid_path("lab"));
        assert!(!valid_path("/a b"));
        assert!(!valid_path("/\"x"));
        assert!(valid_host("localhost"));
        assert!(valid_host("::1"));
        assert!(valid_host("gpu-02.lab"));
        assert!(!valid_host("a b"));
        assert!(!valid_host("x;rm"));
    }

    #[test]
    fn only_web_links_open() {
        assert!(open_link("file:///etc/passwd".into()).is_err());
        assert!(open_link("http://".into()).is_err());
        assert!(open_link("https://a b".into()).is_err());
    }

    #[test]
    fn scripts_do_not_classify_as_servers() {
        for script in [LINUX_SCAN, MAC_SCAN, WINDOWS_SCAN] {
            assert_eq!(classify(script), Classified::Unknown);
        }
    }

    #[test]
    fn tokens_are_stripped_for_assistants() {
        assert_eq!(without_token("/?token=abc"), "/");
        assert_eq!(without_token("/lab?a=1&token=abc&b=2"), "/lab?a=1&b=2");
        assert_eq!(without_token("/tree"), "/tree");
    }
}
