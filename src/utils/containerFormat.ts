import type {
  ContainerAction,
  ContainerInfo,
  ContainerOverview,
  KubePod,
} from "../types/containers";

/** Nothing to show: no runtime and no kubectl on the server. */
export function isOverviewEmpty(overview: ContainerOverview): boolean {
  return overview.runtime == null && overview.kubectl == null;
}

export function isRunning(container: ContainerInfo): boolean {
  return container.state === "running";
}

export function isPodHealthy(pod: KubePod): boolean {
  return (
    pod.status === "Succeeded" ||
    pod.status === "Completed" ||
    (pod.status === "Running" && pod.readyContainers === pod.containers)
  );
}

export function runtimeTitle(runtime: string): string {
  switch (runtime) {
    case "docker":
      return "Docker";
    case "podman":
      return "Podman";
    case "nerdctl":
      return "containerd (nerdctl)";
    default:
      return runtime;
  }
}

/** The card's container line, e.g. `Docker: 1 running / 2 total`. */
export function containerSummary(overview: ContainerOverview): string | null {
  if (overview.runtime == null) return null;
  const title = runtimeTitle(overview.runtime);
  if (overview.runtimeError) return `${title}: not readable`;
  const running = overview.containers.filter(isRunning).length;
  return `${title}: ${running} running / ${overview.containers.length} total`;
}

/** The card's Kubernetes line. */
export function kubeSummary(overview: ContainerOverview): string | null {
  if (overview.kubectl == null) return null;
  const kube = overview.kubernetes;
  if (!kube) return "Kubernetes: unreachable";
  const ready = kube.nodes.filter((node) => node.ready).length;
  const attention = kube.pods.filter((pod) => !isPodHealthy(pod)).length;
  return (
    `Kubernetes: ${ready}/${kube.nodes.length} nodes ready, ${kube.pods.length} pods` +
    (attention > 0 ? `, ${attention} need attention` : "")
  );
}

/** The running container using the most CPU, if any reports CPU. */
export function busiestContainer(overview: ContainerOverview): ContainerInfo | null {
  let busiest: ContainerInfo | null = null;
  for (const container of overview.containers) {
    if (!isRunning(container) || container.cpuPercent == null) continue;
    if (!busiest || container.cpuPercent > (busiest.cpuPercent ?? 0)) {
      busiest = container;
    }
  }
  return busiest;
}

/** `Up 3 hours · CPU 5.0% · Mem 1GiB / 2GiB`. */
export function containerUsageLine(container: ContainerInfo): string {
  const parts = [container.status];
  if (container.cpuPercent != null) parts.push(`CPU ${container.cpuPercent.toFixed(1)}%`);
  if (container.memoryUsage) parts.push(`Mem ${container.memoryUsage}`);
  return parts.join(" · ");
}

/** What each action does, for its confirmation. */
export const CONTAINER_ACTION_EXPLANATIONS: Record<ContainerAction, string> = {
  kill: "Kill sends SIGKILL: the container's processes stop at once without cleaning up.",
  stop: "Stop asks the container to shut down, then kills it after the runtime's timeout.",
  restart: "Restart stops the container and starts it again.",
  start: "Start runs the container again with its existing settings.",
};

export function isDestructiveAction(action: ContainerAction): boolean {
  return action === "kill" || action === "stop";
}

export function actionTitle(action: ContainerAction): string {
  return action.charAt(0).toUpperCase() + action.slice(1);
}
