/** `ContainerOverview` in ssh/container_monitor.rs. */
export type ContainerOverview = {
  /** `docker`, `podman` or `nerdctl`; null when none is installed. */
  runtime: string | null;
  runtimeError?: string;
  containers: ContainerInfo[];
  /** `kubectl`, `k3s kubectl` or `microk8s kubectl`; null when none is installed. */
  kubectl: string | null;
  kubernetes?: KubeOverview;
  kubeError?: string;
};

export type ContainerInfo = {
  /** The full id, used for actions. */
  id: string;
  name: string;
  image: string;
  /** `running`, `exited`, `paused`, `restarting`, `created`, `dead`. */
  state: string;
  /** Human text, e.g. `Up 3 hours (healthy)`. */
  status: string;
  cpuPercent: number | null;
  /** As the runtime prints it, e.g. `512MiB / 15.5GiB`. */
  memoryUsage: string | null;
  memoryPercent: number | null;
};

export type KubeOverview = {
  context: string | null;
  nodes: KubeNode[];
  deployments: KubeDeployment[];
  pods: KubePod[];
  /** Healthy pods beyond the backend's limit that were left out. */
  omittedPods: number;
};

export type KubeNode = {
  name: string;
  ready: boolean;
  roles: string[];
  version: string | null;
};

export type KubeDeployment = {
  namespace: string;
  name: string;
  desired: number;
  ready: number;
  available: number;
};

export type KubePod = {
  namespace: string;
  name: string;
  /** The phase, or a container's waiting reason such as CrashLoopBackOff. */
  status: string;
  readyContainers: number;
  containers: number;
  restarts: number;
  node: string | null;
};

export type ContainerAction = "start" | "stop" | "restart" | "kill";
