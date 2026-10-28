//! TensorBoard and Jupyter servers running on a connected host, and opening
//! one in the local browser through a temporary SSH forward.
//!
//! Detection lists the listening TCP sockets (`ss`, or `/proc/net/tcp` when
//! `ss` is missing), the user's own TensorBoard/Jupyter processes, and
//! Jupyter's runtime files, which carry the port, base URL and login token.
//! Only processes owned by the login user are considered, so on a shared
//! server another user's notebook never shows up.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::State;

use crate::ssh::forwarding::{start_one_forward, ForwardKind, ForwardSpec};
use crate::ssh::resource_details::resolve_remote_os;
use crate::ssh::session::{target_for_active_session, with_ops_session, AppState};
use crate::ssh::system_monitor::{run_remote_command_with_budget, RemoteOs};

const SCAN_TIMEOUT_MS: u32 = 8_000;
const SCAN_BUDGET_SECS: u64 = 5;
/// How long opening waits for a new forward to start listening.
const FORWARD_READY_TIMEOUT: Duration = Duration::from_secs(5);
const TENSORBOARD_DEFAULT_PORT: u16 = 6006;
const MAX_PATH_LEN: usize = 4096;

/// Every row starts with a tag, so a stray line from a login script is ignored.
/// The process filter is written as `[t]ensorboard` and the runtime path is
/// split so that this script's own command line never matches it.
const SCAN_COMMAND: &str = r#"if command -v ss >/dev/null 2>&1; then ss -tlnp 2>/dev/null | sed 's/^/L /'; else awk '$4=="0A"{print "N "$2}' /proc/net/tcp /proc/net/tcp6 2>/dev/null; fi
ps -u "$(id -u)" -o pid=,args= 2>/dev/null | awk '/[t]ensorboard|[j]upyter/{print "P "$0}'
d="${JUPYTER_RUNTIME_DIR:-$HOME/.local/share/jupy""ter/runtime}"
for f in "$d"/jpserver-*.json "$d"/nbserver-*.json; do [ -f "$f" ] && printf 'J %s\n' "$(tr -d '\n' < "$f")"; done
true"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WebServiceKind {
    Tensorboard,
    Jupyter,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebService {
    pub kind: WebServiceKind,
    pub pid: Option<u32>,
    /// Where the server connects for the forward ("localhost" for loopback
    /// and wildcard listeners).
    pub host: String,
    pub port: u16,
    /// Path and query to open, including Jupyter's login token.
    pub path: String,
    /// TensorBoard's logdir or Jupyter's root directory.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebServiceScan {
    /// False on hosts detection does not cover yet (macOS, Windows).
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

/// `127.0.0.1:6006`, `[::1]:8888`, `*:6006`, `0.0.0.0%lo:53`.
fn parse_endpoint(endpoint: &str) -> Option<(String, u16)> {
    let (host, port) = endpoint.rsplit_once(':')?;
    let port = port.parse::<u16>().ok().filter(|port| *port > 0)?;
    Some((forward_host(host), port))
}

/// One `/proc/net/tcp{,6}` local address, e.g. `0100007F:1F90`.
fn parse_proc_endpoint(hex: &str) -> Option<(String, u16)> {
    let (address, port) = hex.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok().filter(|port| *port > 0)?;
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

fn classify(args: &str) -> Option<WebServiceKind> {
    let lower = args.to_ascii_lowercase();
    if lower.contains("ipykernel") {
        return None;
    }
    if lower.contains("tensorboard") {
        return Some(WebServiceKind::Tensorboard);
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
    JUPYTER
        .iter()
        .any(|needle| lower.contains(needle))
        .then_some(WebServiceKind::Jupyter)
}

/// The value of `--flag value` or `--flag=value`.
fn flag_value(args: &str, flag: &str) -> Option<String> {
    let mut words = args.split_whitespace();
    while let Some(word) = words.next() {
        if word == flag {
            return words.next().map(str::to_string);
        }
        if let Some(value) = word.strip_prefix(flag).and_then(|rest| rest.strip_prefix('=')) {
            return Some(value.to_string());
        }
    }
    None
}

fn parse_runtime(json: &str) -> Option<JupyterRuntime> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_string);
    Some(JupyterRuntime {
        pid: value.get("pid").and_then(|v| v.as_u64()).and_then(|v| u32::try_from(v).ok()),
        port: value.get("port").and_then(|v| v.as_u64()).and_then(|v| u16::try_from(v).ok()),
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
        && runtime.token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if token_ok {
        path.push_str("?token=");
        path.push_str(&runtime.token);
    }
    path
}

fn parse_scan(output: &str) -> Vec<WebService> {
    let mut listeners: Vec<Listener> = Vec::new();
    let mut processes: Vec<(u32, String)> = Vec::new();
    let mut runtimes: Vec<JupyterRuntime> = Vec::new();

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
            "N" => {
                if let Some((host, port)) = parse_proc_endpoint(rest.trim()) {
                    listeners.push(Listener { host, port, pids: Vec::new() });
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
            _ => {}
        }
    }

    let listener_on = |port: u16| listeners.iter().find(|listener| listener.port == port);
    let mut found: HashMap<(WebServiceKind, u16), WebService> = HashMap::new();
    let mut add = |service: WebService| {
        found.entry((service.kind, service.port)).or_insert(service);
    };

    for (pid, args) in &processes {
        let Some(kind) = classify(args) else {
            continue;
        };
        let runtime = runtimes.iter().find(|runtime| runtime.pid == Some(*pid));
        let owned: Vec<&Listener> = listeners
            .iter()
            .filter(|listener| listener.pids.contains(pid))
            .collect();
        // Without socket owners (no `ss`, or it hid them), fall back to the
        // port the process says it uses, if something listens there.
        let ports: Vec<(String, u16)> = if !owned.is_empty() {
            owned.iter().map(|listener| (listener.host.clone(), listener.port)).collect()
        } else {
            let port = match kind {
                WebServiceKind::Jupyter => runtime.and_then(|runtime| runtime.port),
                WebServiceKind::Tensorboard => Some(
                    flag_value(args, "--port")
                        .and_then(|port| port.parse().ok())
                        .unwrap_or(TENSORBOARD_DEFAULT_PORT),
                ),
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
            };
            add(WebService { kind, pid: Some(*pid), host, port, path, detail });
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
        });
    }

    let mut services: Vec<WebService> = found.into_values().collect();
    services.sort_by_key(|service| (service.kind, service.port));
    services
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
        return Ok(WebServiceScan { supported: false, services: Vec::new() });
    }
    let target = target_for_active_session(&state, &session_id)?;
    let ops = Arc::clone(&state.ops_sessions);
    let os_cache = Arc::clone(&state.remote_os_cache);
    tauri::async_runtime::spawn_blocking(move || {
        with_ops_session(&ops, &target, SCAN_TIMEOUT_MS, |session| {
            if resolve_remote_os(&os_cache, &target.session_id, session) != RemoteOs::Linux {
                return Ok(WebServiceScan { supported: false, services: Vec::new() });
            }
            run_remote_command_with_budget(session, SCAN_COMMAND, SCAN_BUDGET_SECS).map(
                |output| WebServiceScan { supported: true, services: parse_scan(&output) },
            )
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
    let path = if path.is_empty() { "/".to_string() } else { path };
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
P    4100 /usr/bin/python3 /home/me/.local/bin/tensorboard --logdir runs/exp42 --port 6006
P    5200 /home/me/venv/bin/python /home/me/venv/bin/jupyter-lab --no-browser
P    5300 /home/me/venv/bin/python -m ipykernel_launcher -f /home/me/.local/share/jupyter/runtime/kernel-1.json
J {  \"base_url\": \"/\",  \"pid\": 5200,  \"port\": 8888,  \"root_dir\": \"/home/me/projects\",  \"token\": \"abc123\"}
";

    #[test]
    fn finds_tensorboard_and_jupyter_from_ss() {
        let services = parse_scan(SS_OUTPUT);
        assert_eq!(
            services,
            vec![
                WebService {
                    kind: WebServiceKind::Tensorboard,
                    pid: Some(4100),
                    host: "localhost".into(),
                    port: 6006,
                    path: "/".into(),
                    detail: Some("runs/exp42".into()),
                },
                WebService {
                    kind: WebServiceKind::Jupyter,
                    pid: Some(5200),
                    host: "localhost".into(),
                    port: 8888,
                    path: "/?token=abc123".into(),
                    detail: Some("/home/me/projects".into()),
                },
            ]
        );
    }

    #[test]
    fn falls_back_to_proc_net_tcp_and_flags() {
        let output = "\
N 0100007F:1776
N 00000000:1F90
P 900 python -m tensorboard.main --logdir=/data/logs
P 901 jupyter notebook --port=8080
J {\"pid\": 901, \"port\": 8080, \"base_url\": \"/user/me\", \"token\": \"\"}
";
        let services = parse_scan(output);
        assert_eq!(services.len(), 2);
        assert_eq!(services[0].port, 6006);
        assert_eq!(services[0].host, "localhost");
        assert_eq!(services[0].detail.as_deref(), Some("/data/logs"));
        assert_eq!(services[1].kind, WebServiceKind::Jupyter);
        assert_eq!(services[1].port, 8080);
        assert_eq!(services[1].path, "/user/me/");
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
        assert!(parse_scan("Welcome to Ubuntu\nJ {not json\nL x\n").is_empty());
    }

    #[test]
    fn specific_bind_addresses_are_kept() {
        assert_eq!(parse_endpoint("10.0.0.5:6006"), Some(("10.0.0.5".into(), 6006)));
        assert_eq!(parse_endpoint("*:6006"), Some(("localhost".into(), 6006)));
        assert_eq!(parse_endpoint("[::]:8888"), Some(("localhost".into(), 8888)));
        assert_eq!(parse_endpoint("0.0.0.0%lo:53"), Some(("localhost".into(), 53)));
        assert_eq!(parse_endpoint("127.0.0.1:0"), None);
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
    fn script_does_not_match_its_own_filter() {
        assert!(!SCAN_COMMAND.contains("tensorboard"));
        assert!(!SCAN_COMMAND.contains("jupyter"));
    }
}
