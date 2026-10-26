import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import {
  Boxes,
  CirclePlay,
  CircleStop,
  LoaderCircle,
  RefreshCw,
  RotateCw,
  Zap,
} from "lucide-react";
import { useState, type ReactNode, type RefObject } from "react";
import { ResourceDetailPopover } from "./ResourceDetailPopover";
import { useSessionStore } from "../stores/sessionStore";
import type {
  ContainerAction,
  ContainerInfo,
  ContainerOverview,
  KubeDeployment,
  KubePod,
} from "../types/containers";
import {
  actionTitle,
  CONTAINER_ACTION_EXPLANATIONS,
  containerUsageLine,
  isDestructiveAction,
  isOverviewEmpty,
  isPodHealthy,
  isRunning,
  runtimeTitle,
} from "../utils/containerFormat";

type ContainersPopoverProps = {
  sessionId: string;
  overview: ContainerOverview | null;
  error: string | null;
  loading: boolean;
  onRefresh: () => Promise<void> | void;
  anchorRef: RefObject<HTMLElement | null>;
  onClose: () => void;
};

/**
 * Containers with their usage and actions, and Kubernetes nodes,
 * deployments and pods, on the active server. Every action is confirmed
 * and runs as the SSH user.
 */
export function ContainersPopover({
  sessionId,
  overview,
  error,
  loading,
  onRefresh,
  anchorRef,
  onClose,
}: ContainersPopoverProps) {
  const setMessage = useSessionStore((state) => state.setMessage);
  const [busyId, setBusyId] = useState<string | null>(null);

  const act = async (runtime: string, container: ContainerInfo, action: ContainerAction) => {
    const title = actionTitle(action);
    const accepted = await confirm(CONTAINER_ACTION_EXPLANATIONS[action], {
      title: `${title} ${container.name}?`,
      kind: isDestructiveAction(action) ? "warning" : "info",
      okLabel: title,
    });
    if (!accepted) return;
    setBusyId(container.id);
    try {
      await invoke<string>("container_action", {
        sessionId,
        runtime,
        containerId: container.id,
        action,
      });
      setMessage({ kind: "success", text: `${container.name}: ${action} done` });
    } catch (actionError) {
      setMessage({ kind: "error", text: String(actionError) });
    } finally {
      setBusyId(null);
    }
    await onRefresh();
  };

  const restartDeployment = async (kubectl: string, deployment: KubeDeployment) => {
    const accepted = await confirm(
      "kubectl rollout restart replaces the deployment's pods as its update strategy allows.",
      { title: `Restart ${deployment.name}?`, kind: "warning", okLabel: "Restart" },
    );
    if (!accepted) return;
    setBusyId(`${deployment.namespace}/${deployment.name}`);
    try {
      await invoke<string>("restart_deployment", {
        sessionId,
        kubectl,
        namespace: deployment.namespace,
        deployment: deployment.name,
      });
      setMessage({ kind: "success", text: `${deployment.name} is restarting` });
    } catch (restartError) {
      setMessage({ kind: "error", text: String(restartError) });
    } finally {
      setBusyId(null);
    }
    await onRefresh();
  };

  return (
    <ResourceDetailPopover
      anchorRef={anchorRef}
      ariaLabel="Containers"
      title="Containers"
      icon={<Boxes size={16} />}
      className="containers-popover"
      onClose={onClose}
      headerActions={
        <button
          className="icon-button ghost"
          type="button"
          aria-label="Refresh containers"
          title="Refresh containers"
          disabled={loading}
          onClick={() => void onRefresh()}
        >
          {loading ? <LoaderCircle className="spin" size={14} /> : <RefreshCw size={14} />}
        </button>
      }
    >
      <div className="containers-detail">
        {error && <div className="containers-error" role="alert">{error}</div>}
        {overview ? (
          <>
            {overview.runtime && (
              <section className="containers-section" aria-label={runtimeTitle(overview.runtime)}>
                <h3>{runtimeTitle(overview.runtime)}</h3>
                {overview.runtimeError ? (
                  <p className="containers-problem">{overview.runtimeError}</p>
                ) : overview.containers.length === 0 ? (
                  <p className="containers-muted">No containers</p>
                ) : (
                  overview.containers.map((container) => (
                    <ContainerRow
                      key={container.id}
                      container={container}
                      busy={busyId === container.id}
                      disabled={busyId != null}
                      onAction={(action) => void act(overview.runtime!, container, action)}
                    />
                  ))
                )}
              </section>
            )}
            {overview.kubectl && (
              <KubernetesSection
                overview={overview}
                busyId={busyId}
                onRestart={(deployment) =>
                  void restartDeployment(overview.kubectl!, deployment)
                }
              />
            )}
            {isOverviewEmpty(overview) && (
              <div className="empty-list">No Docker, Podman, nerdctl or kubectl on this server.</div>
            )}
          </>
        ) : (
          !error && (
            <div className="containers-muted" role="status">
              <LoaderCircle className="spin" size={14} /> Looking for Docker, Podman and Kubernetes…
            </div>
          )
        )}
        <p className="containers-footer">Actions run as the SSH user; GpuTerm never uses sudo.</p>
      </div>
    </ResourceDetailPopover>
  );
}

function ContainerRow({
  container,
  busy,
  disabled,
  onAction,
}: {
  container: ContainerInfo;
  busy: boolean;
  disabled: boolean;
  onAction: (action: ContainerAction) => void;
}) {
  const running = isRunning(container);
  return (
    <div className="container-row" data-testid={`container-${container.name}`}>
      <div className="container-row-main">
        <span
          className={`container-state-dot ${running ? "running" : "stopped"}`}
          aria-label={container.state}
          title={container.state}
        />
        <strong title={container.name}>{container.name}</strong>
        {busy && <LoaderCircle className="spin" size={13} />}
      </div>
      <code className="container-image" title={container.image}>{container.image}</code>
      <span className="container-usage">{containerUsageLine(container)}</span>
      <div className="container-actions">
        {running ? (
          <>
            <ActionButton label="Restart" name={container.name} icon={<RotateCw size={12} />} disabled={disabled} onClick={() => onAction("restart")} />
            <ActionButton label="Stop" name={container.name} icon={<CircleStop size={12} />} disabled={disabled} onClick={() => onAction("stop")} />
            <ActionButton label="Kill" name={container.name} icon={<Zap size={12} />} danger disabled={disabled} onClick={() => onAction("kill")} />
          </>
        ) : (
          <ActionButton label="Start" name={container.name} icon={<CirclePlay size={12} />} disabled={disabled} onClick={() => onAction("start")} />
        )}
      </div>
    </div>
  );
}

function ActionButton({
  label,
  name,
  icon,
  danger = false,
  disabled,
  onClick,
}: {
  label: string;
  name: string;
  icon: ReactNode;
  danger?: boolean;
  disabled: boolean;
  onClick: () => void;
}) {
  return (
    <button
      className={`secondary-button compact${danger ? " danger" : ""}`}
      type="button"
      aria-label={`${label} ${name}`}
      disabled={disabled}
      onClick={onClick}
    >
      {icon}
      {label}
    </button>
  );
}

function KubernetesSection({
  overview,
  busyId,
  onRestart,
}: {
  overview: ContainerOverview;
  busyId: string | null;
  onRestart: (deployment: KubeDeployment) => void;
}) {
  const kube = overview.kubernetes;
  return (
    <section className="containers-section" aria-label="Kubernetes">
      <h3>Kubernetes{kube?.context ? ` · ${kube.context}` : ""}</h3>
      {!kube ? (
        <p className="containers-problem">{overview.kubeError ?? "Kubernetes is unavailable"}</p>
      ) : (
        <>
          {kube.nodes.map((node) => (
            <div className="kube-row" key={node.name}>
              <span className={`container-state-dot ${node.ready ? "running" : "failed"}`} />
              <strong>{node.name}</strong>
              <span className="containers-muted">
                {[node.roles.join(","), node.version ?? ""].filter(Boolean).join(" · ")}
              </span>
              <span className={node.ready ? "containers-muted" : "containers-bad"}>
                {node.ready ? "Ready" : "Not ready"}
              </span>
            </div>
          ))}
          {kube.deployments.length > 0 && (
            <>
              <h4>Deployments</h4>
              {kube.deployments.map((deployment) => {
                const key = `${deployment.namespace}/${deployment.name}`;
                return (
                  <div className="kube-row" key={key}>
                    <div className="kube-row-text">
                      <strong>{deployment.name}</strong>
                      <span className={deployment.ready < deployment.desired ? "containers-warn" : "containers-muted"}>
                        {deployment.namespace} · {deployment.ready}/{deployment.desired} ready
                      </span>
                    </div>
                    {busyId === key && <LoaderCircle className="spin" size={13} />}
                    <ActionButton
                      label="Restart"
                      name={deployment.name}
                      icon={<RotateCw size={12} />}
                      disabled={busyId != null}
                      onClick={() => onRestart(deployment)}
                    />
                  </div>
                );
              })}
            </>
          )}
          {kube.pods.length > 0 && (
            <>
              <h4>Pods</h4>
              {kube.pods.map((pod) => (
                <PodRow key={`${pod.namespace}/${pod.name}`} pod={pod} />
              ))}
              {kube.omittedPods > 0 && (
                <p className="containers-muted">{kube.omittedPods} more healthy pods not shown</p>
              )}
            </>
          )}
        </>
      )}
    </section>
  );
}

function PodRow({ pod }: { pod: KubePod }) {
  const healthy = isPodHealthy(pod);
  return (
    <div className="kube-row">
      <span className={`container-state-dot ${healthy ? "running" : "warning"}`} />
      <div className="kube-row-text">
        <span title={pod.name}>{pod.name}</span>
        <span className={healthy ? "containers-muted" : "containers-warn"}>
          {pod.namespace} · {pod.status} · {pod.readyContainers}/{pod.containers} ready ·{" "}
          {pod.restarts} restarts
        </span>
      </div>
    </div>
  );
}
