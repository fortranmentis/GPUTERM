//! Containers (Docker, Podman, nerdctl) and Kubernetes on a connected Linux
//! host, read on demand while the Monitor shows them, plus the few actions
//! GpuTerm offers on them.
//!
//! Everything runs as the SSH user: GpuTerm never uses sudo. A Docker daemon
//! the user may not reach (not in the `docker` group) or a kubeconfig it
//! cannot read is reported, not worked around.

use serde::Serialize;
use serde_json::Value;
use ssh2::Session;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::State;

use super::parse_util::split_sections;
use super::resource_details::resolve_remote_os;
use super::session::{
    target_for_active_session, with_ops_session, AppState, OpsSessions, SshTarget,
};
use super::system_monitor::{run_remote_command_with_budget, shell_quote, RemoteOs};

type OsCache = Arc<Mutex<HashMap<String, RemoteOs>>>;

/// `docker stats --no-stream` samples for about two seconds and each
/// kubectl call may wait up to its own five-second request timeout.
const OVERVIEW_BUDGET_SECS: u64 = 25;
const ACTION_BUDGET_SECS: u64 = 40;
const OPS_TIMEOUT_MS: u32 = 30_000;
/// Busy clusters list thousands of pods; the sheet shows the most relevant.
const MAX_PODS: usize = 300;

/// Finds the first container runtime and kubectl on PATH and prints their
/// state in labelled sections. Errors go to the sections so they can be
/// shown (permission denied, no kubeconfig).
const OVERVIEW_COMMAND: &str = r#"rt=""
for c in docker podman nerdctl; do if command -v "$c" >/dev/null 2>&1; then rt="$c"; break; fi; done
printf '__RUNTIME__\n%s\n' "$rt"
if [ -n "$rt" ]; then
  printf '__PS__\n'
  "$rt" ps -a --no-trunc --format '{{.ID}}	{{.Names}}	{{.Image}}	{{.State}}	{{.Status}}' 2>&1
  printf '__PS_EXIT__\n%s\n' "$?"
  printf '__STATS__\n'
  "$rt" stats --no-stream --format '{{.ID}}	{{.CPUPerc}}	{{.MemUsage}}	{{.MemPerc}}' 2>/dev/null
fi
k=""
if command -v kubectl >/dev/null 2>&1; then k="kubectl"
elif command -v k3s >/dev/null 2>&1; then k="k3s kubectl"
elif command -v microk8s >/dev/null 2>&1; then k="microk8s kubectl"; fi
printf '__KUBECTL__\n%s\n' "$k"
if [ -n "$k" ]; then
  printf '__KUBE_CONTEXT__\n'; $k config current-context 2>&1
  printf '__KUBE_NODES__\n'; $k get nodes -o json --request-timeout=5s 2>&1
  printf '__KUBE_DEPLOYMENTS__\n'; $k get deployments -A -o json --request-timeout=5s 2>&1
  printf '__KUBE_PODS__\n'; $k get pods -A -o json --request-timeout=5s 2>&1
fi
true"#;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerOverview {
    /// `docker`, `podman` or `nerdctl`; None when none is installed.
    pub runtime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_error: Option<String>,
    pub containers: Vec<ContainerInfo>,
    /// The kubectl command used (`kubectl`, `k3s kubectl`, `microk8s kubectl`).
    pub kubectl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kubernetes: Option<KubeOverview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kube_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerInfo {
    /// The full id, used for actions.
    pub id: String,
    pub name: String,
    pub image: String,
    /// `running`, `exited`, `paused`, `restarting`, `created`, `dead`.
    pub state: String,
    /// Human text, e.g. `Up 3 hours (healthy)`.
    pub status: String,
    pub cpu_percent: Option<f64>,
    /// As the runtime prints it, e.g. `512MiB / 15.5GiB`.
    pub memory_usage: Option<String>,
    pub memory_percent: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KubeOverview {
    pub context: Option<String>,
    pub nodes: Vec<KubeNode>,
    pub deployments: Vec<KubeDeployment>,
    pub pods: Vec<KubePod>,
    /// Pods beyond `MAX_PODS` that were left out.
    pub omitted_pods: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KubeNode {
    pub name: String,
    pub ready: bool,
    pub roles: Vec<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KubeDeployment {
    pub namespace: String,
    pub name: String,
    pub desired: u64,
    pub ready: u64,
    pub available: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KubePod {
    pub namespace: String,
    pub name: String,
    /// The phase, or a container's waiting reason such as CrashLoopBackOff.
    pub status: String,
    pub ready_containers: u64,
    pub containers: u64,
    pub restarts: u64,
    pub node: Option<String>,
}

const LINUX_ONLY: &str = "Containers and Kubernetes are read on remote Linux hosts only";

/// The SSH target and caches a container command runs with. Local
/// terminals are refused: the containers are read on remote servers.
fn remote_context(
    state: &AppState,
    session_id: &str,
) -> Result<(SshTarget, OpsSessions, OsCache), String> {
    let is_local = state
        .active_connections
        .lock()
        .map_err(|_| "Active session state is unavailable".to_string())?
        .get(session_id)
        .map(|connection| connection.profile.is_local)
        .ok_or_else(|| "No active terminal session is available".to_string())?;
    if is_local {
        return Err(LINUX_ONLY.to_string());
    }
    Ok((
        target_for_active_session(state, session_id)?,
        Arc::clone(&state.ops_sessions),
        Arc::clone(&state.remote_os_cache),
    ))
}

/// Reads the containers and Kubernetes state of a connected Linux server.
#[tauri::command]
pub async fn container_overview(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<ContainerOverview, String> {
    let (target, ops, os_cache) = remote_context(&state, &session_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        with_ops_session(&ops, &target, OPS_TIMEOUT_MS, |session| {
            require_linux(&os_cache, &target.session_id, session)?;
            run_remote_command_with_budget(session, OVERVIEW_COMMAND, OVERVIEW_BUDGET_SECS)
                .map(|output| parse_overview(&output))
        })
    })
    .await
    .map_err(|error| format!("Container overview task failed: {error}"))?
}

fn require_linux(
    os_cache: &Mutex<HashMap<String, RemoteOs>>,
    session_id: &str,
    session: &Session,
) -> Result<(), String> {
    match resolve_remote_os(os_cache, session_id, session) {
        RemoteOs::Linux => Ok(()),
        _ => Err(LINUX_ONLY.to_string()),
    }
}

pub fn parse_overview(output: &str) -> ContainerOverview {
    let sections = split_sections(output);
    let section = |name: &str| sections.get(name).map(|text| text.trim()).unwrap_or("");
    let mut overview = ContainerOverview::default();

    let runtime = section("RUNTIME");
    if !runtime.is_empty() {
        overview.runtime = Some(runtime.to_string());
        let ps = section("PS");
        if section("PS_EXIT") == "0" {
            overview.containers = parse_ps(ps);
            apply_stats(&mut overview.containers, section("STATS"));
        } else {
            overview.runtime_error = Some(runtime_error(runtime, ps));
        }
    }

    let kubectl = section("KUBECTL");
    if !kubectl.is_empty() {
        overview.kubectl = Some(kubectl.to_string());
        match parse_kube(
            section("KUBE_CONTEXT"),
            section("KUBE_NODES"),
            section("KUBE_DEPLOYMENTS"),
            section("KUBE_PODS"),
        ) {
            Ok(kube) => overview.kubernetes = Some(kube),
            Err(error) => overview.kube_error = Some(error),
        }
    }
    overview
}

fn runtime_error(runtime: &str, output: &str) -> String {
    let lower = output.to_lowercase();
    if lower.contains("permission denied") {
        format!(
            "{runtime} is installed, but this user may not use it. Add the user to the docker group \
             (then sign in again), or use rootless {runtime}."
        )
    } else if lower.contains("cannot connect") || lower.contains("is the docker daemon running") {
        format!("The {runtime} daemon is not running.")
    } else {
        let detail = output.lines().next().unwrap_or("").trim();
        format!("{runtime} ps failed: {detail}")
    }
}

/// Tab-separated `ID Names Image State Status` lines.
pub fn parse_ps(output: &str) -> Vec<ContainerInfo> {
    let mut containers: Vec<ContainerInfo> = output
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 5 || !is_container_id(fields[0]) {
                return None;
            }
            Some(ContainerInfo {
                id: fields[0].to_string(),
                name: fields[1].trim_start_matches('/').to_string(),
                image: fields[2].to_string(),
                state: fields[3].to_lowercase(),
                status: fields[4].to_string(),
                cpu_percent: None,
                memory_usage: None,
                memory_percent: None,
            })
        })
        .collect();
    // Running first, then by name.
    containers.sort_by(|a, b| {
        (a.state != "running")
            .cmp(&(b.state != "running"))
            .then_with(|| a.name.cmp(&b.name))
    });
    containers
}

/// Tab-separated `ID CPU% MemUsage Mem%` lines. Stats print short ids, so
/// they match the full ids from `ps` by prefix.
fn apply_stats(containers: &mut [ContainerInfo], output: &str) {
    for line in output.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 4 || fields[0].len() < 12 {
            continue;
        }
        if let Some(container) = containers.iter_mut().find(|c| c.id.starts_with(fields[0])) {
            container.cpu_percent = parse_percent(fields[1]);
            container.memory_usage = Some(fields[2].trim().to_string())
                .filter(|usage| !usage.is_empty() && usage != "--");
            container.memory_percent = parse_percent(fields[3]);
        }
    }
}

fn parse_percent(value: &str) -> Option<f64> {
    value.trim().trim_end_matches('%').parse().ok()
}

/// 12–64 lowercase hex digits: what the runtimes print and accept.
pub(crate) fn is_container_id(value: &str) -> bool {
    (12..=64).contains(&value.len())
        && value
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

fn parse_kube(
    context: &str,
    nodes: &str,
    deployments: &str,
    pods: &str,
) -> Result<KubeOverview, String> {
    let nodes = kube_json(nodes)?;
    let mut overview = KubeOverview {
        context: Some(context.lines().next().unwrap_or("").trim().to_string())
            .filter(|c| !c.is_empty() && !c.starts_with("error")),
        nodes: items(&nodes).iter().map(parse_node).collect(),
        ..KubeOverview::default()
    };
    // A user who may list nodes may still be denied other resources; keep
    // what was readable.
    if let Ok(deployments) = kube_json(deployments) {
        overview.deployments = items(&deployments).iter().map(parse_deployment).collect();
    }
    if let Ok(pods) = kube_json(pods) {
        let mut parsed: Vec<KubePod> = items(&pods).iter().map(parse_pod).collect();
        parsed.sort_by(|a, b| {
            pod_healthy(a)
                .cmp(&pod_healthy(b))
                .then_with(|| a.namespace.cmp(&b.namespace))
                .then_with(|| a.name.cmp(&b.name))
        });
        overview.omitted_pods = parsed.len().saturating_sub(MAX_PODS);
        parsed.truncate(MAX_PODS);
        overview.pods = parsed;
    }
    Ok(overview)
}

/// kubectl prints JSON on success and a message otherwise.
fn kube_json(output: &str) -> Result<Value, String> {
    serde_json::from_str(output).map_err(|_| {
        let message = output
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("no output")
            .trim();
        if message.contains("connection refused") || message.contains("Unable to connect") {
            "The Kubernetes API server could not be reached.".to_string()
        } else if message.contains("permission denied") || message.contains("Forbidden") {
            format!("kubectl was refused: {message}")
        } else {
            format!("kubectl failed: {message}")
        }
    })
}

fn items(list: &Value) -> Vec<Value> {
    list.get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn text(value: &Value, path: &[&str]) -> Option<String> {
    path.iter()
        .try_fold(value, |value, key| value.get(key))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn number(value: &Value, path: &[&str]) -> u64 {
    path.iter()
        .try_fold(value, |value, key| value.get(key))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn parse_node(node: &Value) -> KubeNode {
    let ready = node
        .pointer("/status/conditions")
        .and_then(Value::as_array)
        .and_then(|conditions| {
            conditions
                .iter()
                .find(|c| c.get("type").and_then(Value::as_str) == Some("Ready"))
        })
        .and_then(|condition| condition.get("status").and_then(Value::as_str))
        == Some("True");
    let mut roles: Vec<String> = node
        .pointer("/metadata/labels")
        .and_then(Value::as_object)
        .map(|labels| {
            labels
                .keys()
                .filter_map(|key| key.strip_prefix("node-role.kubernetes.io/"))
                .filter(|role| !role.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    roles.sort();
    KubeNode {
        name: text(node, &["metadata", "name"]).unwrap_or_default(),
        ready,
        roles,
        version: text(node, &["status", "nodeInfo", "kubeletVersion"]),
    }
}

fn parse_deployment(deployment: &Value) -> KubeDeployment {
    KubeDeployment {
        namespace: text(deployment, &["metadata", "namespace"]).unwrap_or_default(),
        name: text(deployment, &["metadata", "name"]).unwrap_or_default(),
        desired: number(deployment, &["spec", "replicas"]),
        ready: number(deployment, &["status", "readyReplicas"]),
        available: number(deployment, &["status", "availableReplicas"]),
    }
}

fn parse_pod(pod: &Value) -> KubePod {
    let statuses = pod
        .pointer("/status/containerStatuses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let waiting = statuses
        .iter()
        .find_map(|status| text(status, &["state", "waiting", "reason"]));
    let terminated = statuses
        .iter()
        .find_map(|status| text(status, &["state", "terminated", "reason"]));
    let phase = text(pod, &["status", "phase"]).unwrap_or_else(|| "Unknown".to_string());
    let deleting = pod.pointer("/metadata/deletionTimestamp").is_some();
    KubePod {
        namespace: text(pod, &["metadata", "namespace"]).unwrap_or_default(),
        name: text(pod, &["metadata", "name"]).unwrap_or_default(),
        status: if deleting {
            "Terminating".to_string()
        } else {
            waiting
                .or(if phase == "Running" { None } else { terminated })
                .unwrap_or(phase)
        },
        ready_containers: statuses
            .iter()
            .filter(|status| status.get("ready").and_then(Value::as_bool) == Some(true))
            .count() as u64,
        containers: pod
            .pointer("/spec/containers")
            .and_then(Value::as_array)
            .map_or(statuses.len(), Vec::len) as u64,
        restarts: statuses
            .iter()
            .map(|status| number(status, &["restartCount"]))
            .sum(),
        node: text(pod, &["spec", "nodeName"]),
    }
}

fn pod_healthy(pod: &KubePod) -> bool {
    matches!(pod.status.as_str(), "Succeeded" | "Completed")
        || (pod.status == "Running" && pod.ready_containers == pod.containers)
}

// Actions

/// What GpuTerm may do to a container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerAction {
    Start,
    Stop,
    Restart,
    Kill,
}

impl ContainerAction {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "start" => Ok(Self::Start),
            "stop" => Ok(Self::Stop),
            "restart" => Ok(Self::Restart),
            "kill" => Ok(Self::Kill),
            _ => Err(format!("Unsupported container action: {value}")),
        }
    }

    fn verb(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Kill => "kill",
        }
    }
}

/// Only the runtimes the overview reports, by name: never a path or flags.
fn checked_runtime(runtime: &str) -> Result<&'static str, String> {
    match runtime {
        "docker" => Ok("docker"),
        "podman" => Ok("podman"),
        "nerdctl" => Ok("nerdctl"),
        _ => Err(format!("Unknown container runtime: {runtime}")),
    }
}

fn checked_kubectl(kubectl: &str) -> Result<&'static str, String> {
    match kubectl {
        "kubectl" => Ok("kubectl"),
        "k3s kubectl" => Ok("k3s kubectl"),
        "microk8s kubectl" => Ok("microk8s kubectl"),
        _ => Err(format!("Unknown kubectl command: {kubectl}")),
    }
}

/// The command for an action on one container. Container ids are random
/// and never reused, so the id itself is the identity check: a container
/// that was removed and recreated has a new one and the action fails.
pub(crate) fn container_action_command(
    runtime: &str,
    container_id: &str,
    action: ContainerAction,
) -> Result<String, String> {
    let runtime = checked_runtime(runtime)?;
    if !is_container_id(container_id) {
        return Err("That is not a container id".to_string());
    }
    Ok(format!("{runtime} {} {container_id}", action.verb()))
}

/// DNS-1123 names, as Kubernetes requires for namespaces and deployments.
pub(crate) fn is_kube_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        && value.starts_with(|c: char| c.is_ascii_alphanumeric())
        && value.ends_with(|c: char| c.is_ascii_alphanumeric())
}

pub(crate) fn rollout_restart_command(
    kubectl: &str,
    namespace: &str,
    deployment: &str,
) -> Result<String, String> {
    let kubectl = checked_kubectl(kubectl)?;
    if !is_kube_name(namespace) || !is_kube_name(deployment) {
        return Err("That is not a Kubernetes name".to_string());
    }
    Ok(format!(
        "{kubectl} rollout restart deployment/{} -n {} --request-timeout=10s",
        shell_quote(deployment),
        shell_quote(namespace)
    ))
}

/// Starts, stops, restarts or kills a container as the SSH user.
#[tauri::command]
pub async fn container_action(
    state: State<'_, AppState>,
    session_id: String,
    runtime: String,
    container_id: String,
    action: String,
) -> Result<String, String> {
    let command =
        container_action_command(&runtime, &container_id, ContainerAction::parse(&action)?)?;
    run_action(&state, &session_id, command)
        .await
        .map_err(|error| action_error(&runtime, &error))
}

/// `kubectl rollout restart` for one deployment: its pods are replaced one
/// by one as the deployment's update strategy allows.
#[tauri::command]
pub async fn restart_deployment(
    state: State<'_, AppState>,
    session_id: String,
    kubectl: String,
    namespace: String,
    deployment: String,
) -> Result<String, String> {
    let command = rollout_restart_command(&kubectl, &namespace, &deployment)?;
    run_action(&state, &session_id, command).await
}

async fn run_action(state: &AppState, session_id: &str, command: String) -> Result<String, String> {
    let (target, ops, os_cache) = remote_context(state, session_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        with_ops_session(&ops, &target, OPS_TIMEOUT_MS, |session| {
            require_linux(&os_cache, &target.session_id, session)?;
            run_remote_command_with_budget(session, &format!("{command} 2>&1"), ACTION_BUDGET_SECS)
                .map(|output| output.trim().to_string())
        })
    })
    .await
    .map_err(|error| format!("Container action task failed: {error}"))?
}

fn action_error(runtime: &str, error: &str) -> String {
    if error.to_lowercase().contains("permission denied") {
        format!("This user may not control {runtime} containers. GpuTerm does not use sudo.")
    } else if error.contains("No such container") || error.contains("no such container") {
        "The container no longer exists. Refresh the list.".to_string()
    } else {
        error.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID_A: &str = "4f1c0d2e9b8a7f6e5d4c3b2a1f0e9d8c7b6a5f4e3d2c1b0a9f8e7d6c5b4a3f2e";
    const ID_B: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn docker_output() -> String {
        format!(
            "__RUNTIME__\ndocker\n__PS__\n{ID_A}\tvllm\tvllm/vllm-openai:v0.6\trunning\tUp 3 hours (healthy)\n{ID_B}\tbackup\talpine\texited\tExited (0) 2 days ago\n__PS_EXIT__\n0\n__STATS__\n4f1c0d2e9b8a\t182.35%\t31.2GiB / 251.5GiB\t12.41%\n__KUBECTL__\n\n"
        )
    }

    #[test]
    fn reads_docker_containers_with_their_stats() {
        let overview = parse_overview(&docker_output());
        assert_eq!(overview.runtime.as_deref(), Some("docker"));
        assert_eq!(overview.runtime_error, None);
        assert_eq!(overview.kubectl, None);
        assert_eq!(overview.containers.len(), 2);
        let vllm = &overview.containers[0];
        assert_eq!(
            (vllm.name.as_str(), vllm.state.as_str()),
            ("vllm", "running")
        );
        assert_eq!(vllm.cpu_percent, Some(182.35));
        assert_eq!(vllm.memory_usage.as_deref(), Some("31.2GiB / 251.5GiB"));
        assert_eq!(vllm.memory_percent, Some(12.41));
        let backup = &overview.containers[1];
        assert_eq!(backup.state, "exited");
        assert_eq!(backup.cpu_percent, None);
    }

    #[test]
    fn explains_a_daemon_the_user_may_not_use() {
        let output = "__RUNTIME__\ndocker\n__PS__\npermission denied while trying to connect to the Docker daemon socket at unix:///var/run/docker.sock\n__PS_EXIT__\n1\n__KUBECTL__\n\n";
        let overview = parse_overview(output);
        assert!(overview.containers.is_empty());
        assert!(overview.runtime_error.unwrap().contains("docker group"));
    }

    #[test]
    fn a_host_without_runtimes_reports_none() {
        let overview = parse_overview("__RUNTIME__\n\n__KUBECTL__\n\n");
        assert_eq!(overview, ContainerOverview::default());
    }

    #[test]
    fn reads_kubernetes_nodes_deployments_and_pods() {
        let nodes = r#"{"items":[{"metadata":{"name":"gpu-1","labels":{"node-role.kubernetes.io/control-plane":"","kubernetes.io/os":"linux"}},"status":{"conditions":[{"type":"MemoryPressure","status":"False"},{"type":"Ready","status":"True"}],"nodeInfo":{"kubeletVersion":"v1.31.2"}}},{"metadata":{"name":"gpu-2"},"status":{"conditions":[{"type":"Ready","status":"Unknown"}]}}]}"#;
        let deployments = r#"{"items":[{"metadata":{"namespace":"llm","name":"vllm"},"spec":{"replicas":2},"status":{"readyReplicas":1,"availableReplicas":1}}]}"#;
        let pods = r#"{"items":[
            {"metadata":{"namespace":"llm","name":"vllm-a"},"spec":{"nodeName":"gpu-1","containers":[{},{}]},"status":{"phase":"Running","containerStatuses":[{"ready":true,"restartCount":0,"state":{"running":{}}},{"ready":true,"restartCount":1,"state":{"running":{}}}]}},
            {"metadata":{"namespace":"llm","name":"vllm-b"},"spec":{"nodeName":"gpu-2","containers":[{}]},"status":{"phase":"Running","containerStatuses":[{"ready":false,"restartCount":7,"state":{"waiting":{"reason":"CrashLoopBackOff"}}}]}},
            {"metadata":{"namespace":"jobs","name":"train-1"},"spec":{"containers":[{}]},"status":{"phase":"Succeeded","containerStatuses":[{"ready":false,"restartCount":0,"state":{"terminated":{"reason":"Completed"}}}]}}
        ]}"#;
        let output = format!(
            "__RUNTIME__\n\n__KUBECTL__\nkubectl\n__KUBE_CONTEXT__\nlab\n__KUBE_NODES__\n{nodes}\n__KUBE_DEPLOYMENTS__\n{deployments}\n__KUBE_PODS__\n{pods}\n"
        );
        let overview = parse_overview(&output);
        assert_eq!(overview.kube_error, None);
        let kube = overview.kubernetes.unwrap();
        assert_eq!(kube.context.as_deref(), Some("lab"));
        assert_eq!(kube.nodes.len(), 2);
        assert!(kube.nodes[0].ready);
        assert_eq!(kube.nodes[0].roles, vec!["control-plane"]);
        assert_eq!(kube.nodes[0].version.as_deref(), Some("v1.31.2"));
        assert!(!kube.nodes[1].ready);
        assert_eq!(
            kube.deployments[0],
            KubeDeployment {
                namespace: "llm".into(),
                name: "vllm".into(),
                desired: 2,
                ready: 1,
                available: 1,
            }
        );
        // Unhealthy pods come first.
        assert_eq!(kube.pods[0].name, "vllm-b");
        assert_eq!(kube.pods[0].status, "CrashLoopBackOff");
        assert_eq!(kube.pods[0].restarts, 7);
        let healthy = kube.pods.iter().find(|pod| pod.name == "vllm-a").unwrap();
        assert_eq!(
            (
                healthy.ready_containers,
                healthy.containers,
                healthy.restarts
            ),
            (2, 2, 1)
        );
        let job = kube.pods.iter().find(|pod| pod.name == "train-1").unwrap();
        // As `kubectl get pods` shows a finished job.
        assert_eq!(job.status, "Completed");
    }

    #[test]
    fn reports_an_unreachable_cluster() {
        let output = "__RUNTIME__\n\n__KUBECTL__\nk3s kubectl\n__KUBE_CONTEXT__\nerror: current-context is not set\n__KUBE_NODES__\nThe connection to the server localhost:8080 was refused - did you specify the right host or port?\n__KUBE_DEPLOYMENTS__\n__KUBE_PODS__\n";
        let overview = parse_overview(output);
        assert_eq!(overview.kubectl.as_deref(), Some("k3s kubectl"));
        assert!(overview.kubernetes.is_none());
        assert!(overview.kube_error.unwrap().contains("kubectl failed"));
    }

    /// Runs the real overview script against fake docker and kubectl.
    #[cfg(unix)]
    #[test]
    fn the_overview_script_reads_fake_tools() {
        use std::os::unix::fs::PermissionsExt as _;
        let bin = std::env::temp_dir().join(format!("gputerm-ctr-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&bin).unwrap();
        let docker = format!(
            "#!/bin/sh\ncase \"$1\" in\nps) printf '{ID_A}\\tvllm\\timg\\trunning\\tUp 1 hour\\n' ;;\nstats) printf '4f1c0d2e9b8a\\t5.00%%\\t1GiB / 2GiB\\t50.00%%\\n' ;;\nesac\n"
        );
        let kubectl = "#!/bin/sh\ncase \"$1 $2\" in\n'config current-context') echo lab ;;\n'get nodes') echo '{\"items\":[]}' ;;\n'get deployments') echo '{\"items\":[]}' ;;\n'get pods') echo '{\"items\":[]}' ;;\nesac\n";
        for (name, body) in [("docker", docker.as_str()), ("kubectl", kubectl)] {
            let path = bin.join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(OVERVIEW_COMMAND)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output()
            .unwrap();
        let overview = parse_overview(&String::from_utf8(output.stdout).unwrap());
        std::fs::remove_dir_all(&bin).unwrap();
        assert_eq!(overview.runtime.as_deref(), Some("docker"));
        assert_eq!(overview.containers.len(), 1);
        assert_eq!(overview.containers[0].cpu_percent, Some(5.0));
        assert_eq!(overview.kubectl.as_deref(), Some("kubectl"));
        assert_eq!(overview.kubernetes.unwrap().context.as_deref(), Some("lab"));
    }

    #[test]
    fn actions_accept_only_known_runtimes_and_real_ids() {
        assert_eq!(
            container_action_command("docker", ID_A, ContainerAction::Restart).unwrap(),
            format!("docker restart {ID_A}")
        );
        assert_eq!(
            container_action_command("podman", "4f1c0d2e9b8a", ContainerAction::Stop).unwrap(),
            "podman stop 4f1c0d2e9b8a"
        );
        for (runtime, id) in [
            ("docker; rm -rf ~", ID_A),
            ("/usr/bin/docker", ID_A),
            ("docker", "vllm"),
            ("docker", "4f1c0d2e9b8a; reboot"),
            ("docker", "4F1C0D2E9B8A"),
            ("docker", "abc"),
        ] {
            assert!(
                container_action_command(runtime, id, ContainerAction::Kill).is_err(),
                "{runtime} {id}"
            );
        }
        assert!(ContainerAction::parse("rm").is_err());
    }

    #[test]
    fn rollout_restarts_quote_valid_names_only() {
        assert_eq!(
            rollout_restart_command("kubectl", "llm", "vllm-server").unwrap(),
            "kubectl rollout restart deployment/'vllm-server' -n 'llm' --request-timeout=10s"
        );
        assert!(rollout_restart_command("kubectl", "llm", "Vllm").is_err());
        assert!(rollout_restart_command("kubectl", "llm", "a;b").is_err());
        assert!(rollout_restart_command("kubectl", "-n", "x").is_err());
        assert!(rollout_restart_command("sudo kubectl", "llm", "vllm").is_err());
    }
}
