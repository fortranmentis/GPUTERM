import { describe, expect, it } from "vitest";
import type { ContainerOverview } from "../types/containers";
import {
  busiestContainer,
  containerSummary,
  containerUsageLine,
  isOverviewEmpty,
  isPodHealthy,
  kubeSummary,
} from "./containerFormat";

/** As `container_overview` serialises it. */
const SAMPLE_OVERVIEW: ContainerOverview = {
  runtime: "docker",
  containers: [
    {
      id: "4f1c0d2e9b8a",
      name: "vllm",
      image: "vllm/vllm-openai",
      state: "running",
      status: "Up 3 hours",
      cpuPercent: 182.3,
      memoryUsage: "31GiB / 251GiB",
      memoryPercent: 12.4,
    },
    {
      id: "0123456789ab",
      name: "backup",
      image: "alpine",
      state: "exited",
      status: "Exited (0)",
      cpuPercent: null,
      memoryUsage: null,
      memoryPercent: null,
    },
  ],
  kubectl: "kubectl",
  kubernetes: {
    context: "lab",
    nodes: [
      { name: "gpu-1", ready: true, roles: ["control-plane"], version: "v1.31.2" },
      { name: "gpu-2", ready: false, roles: [], version: null },
    ],
    deployments: [{ namespace: "llm", name: "vllm", desired: 2, ready: 1, available: 1 }],
    pods: [
      {
        namespace: "llm",
        name: "vllm-b",
        status: "CrashLoopBackOff",
        readyContainers: 0,
        containers: 1,
        restarts: 7,
        node: "gpu-2",
      },
      {
        namespace: "jobs",
        name: "train",
        status: "Completed",
        readyContainers: 0,
        containers: 1,
        restarts: 0,
        node: null,
      },
    ],
    omittedPods: 0,
  },
};

describe("containerFormat", () => {
  it("summarises containers and Kubernetes for the card", () => {
    expect(isOverviewEmpty(SAMPLE_OVERVIEW)).toBe(false);
    expect(containerSummary(SAMPLE_OVERVIEW)).toBe("Docker: 1 running / 2 total");
    expect(kubeSummary(SAMPLE_OVERVIEW)).toBe(
      "Kubernetes: 1/2 nodes ready, 2 pods, 1 need attention",
    );
    expect(isPodHealthy(SAMPLE_OVERVIEW.kubernetes!.pods[1])).toBe(true);
    expect(busiestContainer(SAMPLE_OVERVIEW)?.name).toBe("vllm");
    expect(containerUsageLine(SAMPLE_OVERVIEW.containers[0])).toBe(
      "Up 3 hours · CPU 182.3% · Mem 31GiB / 251GiB",
    );
  });

  it("reports a server without runtimes as empty and unreadable ones as such", () => {
    const none: ContainerOverview = { runtime: null, containers: [], kubectl: null };
    expect(isOverviewEmpty(none)).toBe(true);
    expect(containerSummary(none)).toBeNull();
    expect(kubeSummary(none)).toBeNull();
    const denied: ContainerOverview = {
      runtime: "docker",
      runtimeError: "no access",
      containers: [],
      kubectl: "kubectl",
      kubeError: "down",
    };
    expect(containerSummary(denied)).toBe("Docker: not readable");
    expect(kubeSummary(denied)).toBe("Kubernetes: unreachable");
    expect(busiestContainer(denied)).toBeNull();
  });
});
