import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { LlmRuntimeDetailContent } from "./LlmRuntimePopover";
import type {
  LlmInstance,
  LlmInstanceTelemetry,
  LlmRuntimeMetrics,
  LlmTelemetry,
} from "../types/llm";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);

const NOW = 1_799_000_000;

function ollamaInstance(overrides: Partial<LlmInstance> = {}): LlmInstance {
  return {
    id: "inst-ollama",
    name: "GPU server Ollama",
    runtimeType: "ollama",
    baseUrl: "http://192.168.0.20:11434",
    enabled: true,
    requestTimeoutMs: 3000,
    pollIntervalSecs: 5,
    createdAt: NOW,
    updatedAt: NOW,
    ...overrides,
  };
}

function vllmInstance(overrides: Partial<LlmInstance> = {}): LlmInstance {
  return {
    id: "inst-vllm",
    name: "GPU server vLLM",
    runtimeType: "vllm",
    baseUrl: "http://192.168.0.21:8000",
    enabled: true,
    requestTimeoutMs: 3000,
    pollIntervalSecs: 5,
    createdAt: NOW,
    updatedAt: NOW,
    ...overrides,
  };
}

function metrics(overrides: Partial<LlmRuntimeMetrics> = {}): LlmRuntimeMetrics {
  return {
    requestsRunning: 3,
    requestsWaiting: 2,
    requestsSwapped: null,
    kvCacheUsageRatio: 0.42,
    kvCacheRemainingRatio: 0.58,
    prefixCacheHitRatio: 0.25,
    promptTokensPerSecond: 60,
    generationTokensPerSecond: 10,
    requestsPerSecond: 2,
    preemptionsTotal: 7,
    preemptionsDelta: 0,
    ttftP50Seconds: 0.42,
    ttftP95Seconds: 1,
    e2eLatencyP95Seconds: null,
    queueTimeP95Seconds: null,
    collectedAt: NOW,
    unsupported: ["vllm:num_requests_swapped"],
    ...overrides,
  };
}

function entry(
  instance: LlmInstance,
  overrides: Partial<LlmInstanceTelemetry> = {},
): LlmInstanceTelemetry {
  return {
    instance,
    hasApiKey: false,
    status: {
      instanceId: instance.id,
      runtimeType: instance.runtimeType,
      status: "online",
      responseTimeMs: 12,
      checkedAt: NOW,
    },
    severity: "normal",
    severityReasons: [],
    models: [],
    runningModelCount: 0,
    metrics: null,
    history: [],
    events: [],
    lastSuccessAt: NOW,
    lastError: null,
    consecutiveFailures: 0,
    ...overrides,
  };
}

function telemetryFor(entries: LlmInstanceTelemetry[]): LlmTelemetry {
  return {
    generatedAt: NOW,
    summary: {
      registered: entries.length,
      enabled: entries.length,
      normal: entries.filter((item) => item.severity === "normal").length,
      warning: entries.filter(
        (item) => item.severity === "warning" || item.severity === "congested",
      ).length,
      error: entries.filter((item) => item.severity === "critical").length,
      unknown: entries.filter((item) => item.severity === "unknown").length,
      models: entries.reduce((total, item) => total + item.runningModelCount, 0),
      vllmRequestsRunning: null,
      vllmRequestsWaiting: null,
    },
    instances: entries,
  };
}

describe("LlmRuntimeDetailContent", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue([]);
  });

  it("explains that nothing is polled when no instance is registered", () => {
    render(
      <LlmRuntimeDetailContent
        telemetry={null}
        instances={[]}
        onInstancesChange={() => undefined}
      />,
    );

    expect(screen.getByText("No LLM runtimes registered")).toBeInTheDocument();
    expect(screen.getByText(/Nothing is polled until you do/)).toBeInTheDocument();
  });

  it("shows a registered instance before its first poll produces telemetry", () => {
    render(
      <LlmRuntimeDetailContent
        telemetry={null}
        instances={[ollamaInstance()]}
        onInstancesChange={() => undefined}
      />,
    );

    // Once in the instance strip, once in the detail heading.
    expect(screen.getAllByText("GPU server Ollama")).toHaveLength(2);
    // Never polled is "not polled yet", not a healthy reading.
    expect(screen.getByText("Not polled yet")).toBeInTheDocument();
  });

  it("renders vLLM metrics and labels the KV cache as server-wide", () => {
    const instance = vllmInstance();
    render(
      <LlmRuntimeDetailContent
        telemetry={telemetryFor([entry(instance, { metrics: metrics() })])}
        instances={[instance]}
        onInstancesChange={() => undefined}
      />,
    );

    expect(screen.getByText("Server-wide KV cache")).toBeInTheDocument();
    expect(screen.getByText("42.0%")).toBeInTheDocument();
    expect(
      screen.getByText(/not the remaining context of any one conversation/),
    ).toBeInTheDocument();
    expect(screen.getByText("60.0 tok/s")).toBeInTheDocument();
    expect(screen.getByText("2.0 req/s")).toBeInTheDocument();
  });

  it("marks a metric this version omits as unsupported rather than zero", () => {
    const instance = vllmInstance();
    render(
      <LlmRuntimeDetailContent
        telemetry={telemetryFor([entry(instance, { metrics: metrics() })])}
        instances={[instance]}
        onInstancesChange={() => undefined}
      />,
    );

    const swapped = screen.getByText("Swapped requests").parentElement;
    expect(within(swapped as HTMLElement).getByText("not supported")).toBeInTheDocument();
    expect(screen.getByText("Not exposed by this server")).toBeInTheDocument();
    expect(screen.getByText("vllm:num_requests_swapped")).toBeInTheDocument();
  });

  it("shows an em dash rather than 0 for a reading the server did not report", () => {
    const instance = vllmInstance();
    render(
      <LlmRuntimeDetailContent
        telemetry={telemetryFor([
          entry(instance, {
            metrics: metrics({
              prefixCacheHitRatio: null,
              ttftP50Seconds: null,
              promptTokensPerSecond: null,
            }),
          }),
        ])}
        instances={[instance]}
        onInstancesChange={() => undefined}
      />,
    );

    for (const label of ["Prefix cache hit rate", "TTFT P50", "Prompt tokens"]) {
      const field = screen.getByText(label).parentElement as HTMLElement;
      expect(within(field).getByText("—")).toBeInTheDocument();
    }
  });

  it("surfaces a connection failure with its cause", () => {
    const instance = ollamaInstance();
    render(
      <LlmRuntimeDetailContent
        telemetry={telemetryFor([
          entry(instance, {
            severity: "critical",
            severityReasons: ["repeated_failures"],
            status: {
              instanceId: instance.id,
              runtimeType: "ollama",
              status: "offline",
              responseTimeMs: null,
              checkedAt: NOW,
              errorCode: "connection_refused",
              errorMessage: "The server refused the connection.",
            },
            consecutiveFailures: 3,
            lastError: {
              at: NOW,
              code: "connection_refused",
              message: "The server refused the connection.",
            },
          }),
        ])}
        instances={[instance]}
        onInstancesChange={() => undefined}
      />,
    );

    expect(screen.getByText("Connection refused")).toBeInTheDocument();
    expect(
      screen.getByText("Three or more consecutive failures"),
    ).toBeInTheDocument();
    expect(screen.getByText("Critical")).toBeInTheDocument();
  });

  it("labels Ollama model fields without claiming RAM or live context", () => {
    const instance = ollamaInstance();
    render(
      <LlmRuntimeDetailContent
        telemetry={telemetryFor([
          entry(instance, {
            runningModelCount: 1,
            models: [
              {
                id: "llama3:70b",
                name: "llama3:70b",
                status: "running",
                parameterSize: "70B",
                quantization: "Q4_0",
                modelSizeBytes: 40_000_000_000,
                vramSizeBytes: 30_000_000_000,
                vramResidentPercent: 75,
                nonVramBytes: 10_000_000_000,
                contextLength: 8192,
                expiresAt: NOW + 300,
                expiresInSeconds: 300,
              },
            ],
          }),
        ])}
        instances={[instance]}
        onInstancesChange={() => undefined}
      />,
    );

    expect(screen.getByText("Estimated non-VRAM residency")).toBeInTheDocument();
    expect(screen.getByText("Configured max context")).toBeInTheDocument();
    expect(screen.queryByText(/RAM usage/)).not.toBeInTheDocument();
    expect(screen.getByText("5m")).toBeInTheDocument();
  });

  it("switching an instance off calls the backend and keeps polling the others", async () => {
    const ollama = ollamaInstance();
    const vllm = vllmInstance();
    const onInstancesChange = vi.fn();
    mockInvoke.mockResolvedValue([{ ...ollama, enabled: false }, vllm]);

    render(
      <LlmRuntimeDetailContent
        telemetry={telemetryFor([entry(ollama), entry(vllm)])}
        instances={[ollama, vllm]}
        onInstancesChange={onInstancesChange}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Disable" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("set_llm_instance_enabled", {
        id: "inst-ollama",
        enabled: false,
      });
    });
    expect(onInstancesChange).toHaveBeenCalledWith([
      { ...ollama, enabled: false },
      vllm,
    ]);
  });

  it("reports a failed backend call instead of silently doing nothing", async () => {
    const instance = ollamaInstance();
    mockInvoke.mockRejectedValue("Instance list is unavailable");

    render(
      <LlmRuntimeDetailContent
        telemetry={telemetryFor([entry(instance)])}
        instances={[instance]}
        onInstancesChange={() => undefined}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    expect(
      await screen.findByText("Instance list is unavailable"),
    ).toBeInTheDocument();
  });

  it("names the SSH hop so a bare 127.0.0.1 is never ambiguous", () => {
    const tunneled = ollamaInstance({
      baseUrl: "http://127.0.0.1:11434",
      sshProfileId: "wsl",
    });
    render(
      <LlmRuntimeDetailContent
        telemetry={telemetryFor([
          entry(tunneled, { sshProfileName: "Wsl" }),
        ])}
        instances={[tunneled]}
        onInstancesChange={() => undefined}
      />,
    );

    expect(screen.getByText("via Wsl")).toBeInTheDocument();
    const reached = screen.getByText("Reached through").parentElement as HTMLElement;
    expect(within(reached).getByText("Wsl")).toBeInTheDocument();
  });

  it("shows Direct for an instance polled without a tunnel", () => {
    const direct = ollamaInstance();
    render(
      <LlmRuntimeDetailContent
        telemetry={telemetryFor([entry(direct)])}
        instances={[direct]}
        onInstancesChange={() => undefined}
      />,
    );

    const reached = screen.getByText("Reached through").parentElement as HTMLElement;
    expect(within(reached).getByText("Direct")).toBeInTheDocument();
    expect(screen.queryByText(/^via /)).not.toBeInTheDocument();
  });

  it("explains an untrusted SSH host key instead of blaming the runtime", () => {
    const tunneled = ollamaInstance({
      baseUrl: "http://127.0.0.1:11434",
      sshProfileId: "wsl",
    });
    render(
      <LlmRuntimeDetailContent
        telemetry={telemetryFor([
          entry(tunneled, {
            severity: "critical",
            severityReasons: ["ssh_unreachable"],
            sshProfileName: "Wsl",
            status: {
              instanceId: tunneled.id,
              runtimeType: "ollama",
              status: "offline",
              responseTimeMs: null,
              checkedAt: NOW,
              errorCode: "ssh_host_untrusted",
              errorMessage:
                "Cannot verify the SSH host key for 100.74.103.17:22. Open that SSH session in a terminal once, or use Test connection on this instance, to review and trust its fingerprint.",
            },
            lastError: {
              at: NOW,
              code: "ssh_host_untrusted",
              message:
                "Cannot verify the SSH host key for 100.74.103.17:22. Open that SSH session in a terminal once, or use Test connection on this instance, to review and trust its fingerprint.",
            },
          }),
        ])}
        instances={[tunneled]}
        onInstancesChange={() => undefined}
      />,
    );

    expect(screen.getByText("SSH host key not trusted")).toBeInTheDocument();
    expect(
      screen.getByText("The SSH tunnel could not be established"),
    ).toBeInTheDocument();
    // The actionable instruction has to be visible, not just the code.
    expect(screen.getByText(/Test connection on this instance/)).toBeInTheDocument();
  });

  it("explains a refused loopback poll that has no tunnel", () => {
    const direct = ollamaInstance({ baseUrl: "http://127.0.0.1:11434" });
    render(
      <LlmRuntimeDetailContent
        telemetry={telemetryFor([
          entry(direct, {
            severity: "critical",
            severityReasons: ["repeated_failures"],
            status: {
              instanceId: direct.id,
              runtimeType: "ollama",
              status: "offline",
              responseTimeMs: null,
              checkedAt: NOW,
              errorCode: "connection_refused",
              errorMessage: "The server refused the connection.",
            },
            consecutiveFailures: 6,
          }),
        ])}
        instances={[direct]}
        onInstancesChange={() => undefined}
      />,
    );

    // "Connection refused" alone gives no hint that the runtime is elsewhere.
    expect(
      screen.getByText(/Reached through is/),
    ).toBeInTheDocument();
    expect(screen.getByText(/set\s+Reach through to that host/)).toBeInTheDocument();
  });

  it("does not offer the tunnel hint when it would be wrong advice", () => {
    // Already tunneled: the address is meant to be the remote loopback.
    const tunneled = ollamaInstance({
      baseUrl: "http://127.0.0.1:11434",
      sshProfileId: "wsl",
    });
    const { unmount } = render(
      <LlmRuntimeDetailContent
        telemetry={telemetryFor([
          entry(tunneled, {
            severity: "critical",
            sshProfileName: "Wsl",
            status: {
              instanceId: tunneled.id,
              runtimeType: "ollama",
              status: "offline",
              responseTimeMs: null,
              checkedAt: NOW,
              errorCode: "connection_refused",
            },
          }),
        ])}
        instances={[tunneled]}
        onInstancesChange={() => undefined}
      />,
    );
    expect(screen.queryByText(/Reached through is/)).not.toBeInTheDocument();
    unmount();

    // A healthy local runtime is a legitimate setup, so no scolding.
    const healthyLocal = ollamaInstance({ baseUrl: "http://127.0.0.1:11434" });
    render(
      <LlmRuntimeDetailContent
        telemetry={telemetryFor([entry(healthyLocal)])}
        instances={[healthyLocal]}
        onInstancesChange={() => undefined}
      />,
    );
    expect(screen.queryByText(/Reached through is/)).not.toBeInTheDocument();
  });

  it("records a counter reset as a recent change", () => {
    const instance = vllmInstance();
    render(
      <LlmRuntimeDetailContent
        telemetry={telemetryFor([
          entry(instance, {
            events: [{ at: NOW, kind: "counters_reset", code: "counters_reset" }],
          }),
        ])}
        instances={[instance]}
        onInstancesChange={() => undefined}
      />,
    );

    expect(
      screen.getByText(/Counters restarted — the server was probably restarted/),
    ).toBeInTheDocument();
  });
});
