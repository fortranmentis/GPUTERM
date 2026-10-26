import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createRef } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import { ContainersPopover } from "./ContainersPopover";
import { useSessionStore } from "../stores/sessionStore";
import type { ContainerOverview } from "../types/containers";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  confirm: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);
const mockConfirm = vi.mocked(confirm);

const OVERVIEW: ContainerOverview = {
  runtime: "docker",
  containers: [
    {
      id: "4f1c0d2e9b8a",
      name: "vllm",
      image: "vllm/vllm-openai",
      state: "running",
      status: "Up 3 hours",
      cpuPercent: 5,
      memoryUsage: "1GiB / 2GiB",
      memoryPercent: 50,
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
  kubectl: "k3s kubectl",
  kubernetes: {
    context: "lab",
    nodes: [{ name: "gpu-1", ready: true, roles: ["control-plane"], version: "v1.31.2" }],
    deployments: [{ namespace: "llm", name: "vllm", desired: 2, ready: 1, available: 1 }],
    pods: [
      {
        namespace: "llm",
        name: "vllm-b",
        status: "CrashLoopBackOff",
        readyContainers: 0,
        containers: 1,
        restarts: 7,
        node: "gpu-1",
      },
    ],
    omittedPods: 4,
  },
};

function renderPopover(onRefresh = vi.fn()) {
  render(
    <ContainersPopover
      sessionId="session-1"
      overview={OVERVIEW}
      error={null}
      loading={false}
      onRefresh={onRefresh}
      anchorRef={createRef<HTMLElement>()}
      onClose={vi.fn()}
    />,
  );
  return screen.getByRole("dialog", { name: "Containers" });
}

describe("ContainersPopover", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockConfirm.mockReset();
    mockInvoke.mockResolvedValue("");
    useSessionStore.setState({ message: null });
  });

  it("lists containers, Kubernetes and the sudo note", () => {
    const dialog = renderPopover();
    const vllm = within(dialog).getByTestId("container-vllm");
    expect(vllm).toHaveTextContent("vllm/vllm-openai");
    expect(vllm).toHaveTextContent("Up 3 hours · CPU 5.0% · Mem 1GiB / 2GiB");
    for (const action of ["Restart", "Stop", "Kill"]) {
      expect(within(vllm).getByRole("button", { name: `${action} vllm` })).toBeInTheDocument();
    }
    const backup = within(dialog).getByTestId("container-backup");
    expect(within(backup).getAllByRole("button")).toHaveLength(1);
    expect(within(backup).getByRole("button", { name: "Start backup" })).toBeInTheDocument();

    expect(dialog).toHaveTextContent("Kubernetes · lab");
    expect(dialog).toHaveTextContent("llm · 1/2 ready");
    expect(dialog).toHaveTextContent("llm · CrashLoopBackOff · 0/1 ready · 7 restarts");
    expect(dialog).toHaveTextContent("4 more healthy pods not shown");
    expect(dialog).toHaveTextContent("Actions run as the SSH user; GpuTerm never uses sudo.");
  });

  it("kills a container only after a destructive confirmation", async () => {
    const onRefresh = vi.fn();
    const dialog = renderPopover(onRefresh);
    mockConfirm.mockResolvedValueOnce(false);
    fireEvent.click(within(dialog).getByRole("button", { name: "Kill vllm" }));
    await waitFor(() => expect(mockConfirm).toHaveBeenCalledTimes(1));
    expect(mockConfirm).toHaveBeenCalledWith(
      expect.stringContaining("SIGKILL"),
      expect.objectContaining({ title: "Kill vllm?", kind: "warning", okLabel: "Kill" }),
    );
    expect(mockInvoke).not.toHaveBeenCalled();

    mockConfirm.mockResolvedValueOnce(true);
    fireEvent.click(within(dialog).getByRole("button", { name: "Kill vllm" }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("container_action", {
        sessionId: "session-1",
        runtime: "docker",
        containerId: "4f1c0d2e9b8a",
        action: "kill",
      }),
    );
    await waitFor(() => expect(onRefresh).toHaveBeenCalled());
    expect(useSessionStore.getState().message?.kind).toBe("success");
  });

  it("starts a stopped container with a non-destructive confirmation", async () => {
    const dialog = renderPopover();
    mockConfirm.mockResolvedValueOnce(true);
    fireEvent.click(within(dialog).getByRole("button", { name: "Start backup" }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith(
        "container_action",
        expect.objectContaining({ containerId: "0123456789ab", action: "start" }),
      ),
    );
    expect(mockConfirm).toHaveBeenCalledWith(
      expect.stringContaining("existing settings"),
      expect.objectContaining({ kind: "info", okLabel: "Start" }),
    );
  });

  it("restarts a deployment with the kubectl the overview found", async () => {
    const dialog = renderPopover();
    mockConfirm.mockResolvedValueOnce(true);
    const deployments = within(dialog).getAllByRole("button", { name: "Restart vllm" });
    // The container's button comes first, the deployment's second.
    fireEvent.click(deployments[1]);
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("restart_deployment", {
        sessionId: "session-1",
        kubectl: "k3s kubectl",
        namespace: "llm",
        deployment: "vllm",
      }),
    );
    expect(mockConfirm).toHaveBeenCalledWith(
      expect.stringContaining("rollout restart"),
      expect.objectContaining({ title: "Restart vllm?" }),
    );
  });

  it("reports a failed action", async () => {
    const dialog = renderPopover();
    mockConfirm.mockResolvedValueOnce(true);
    mockInvoke.mockRejectedValueOnce("This user may not control docker containers.");
    fireEvent.click(within(dialog).getByRole("button", { name: "Stop vllm" }));
    await waitFor(() =>
      expect(useSessionStore.getState().message).toEqual({
        kind: "error",
        text: "This user may not control docker containers.",
      }),
    );
  });
});
