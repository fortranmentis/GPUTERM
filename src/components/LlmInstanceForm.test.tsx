import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import { LlmInstanceForm } from "./LlmInstanceForm";
import type { LlmInstance } from "../types/llm";
import type { SessionProfile } from "../types/session";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  confirm: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);
const mockConfirm = vi.mocked(confirm);

const profiles: SessionProfile[] = [
  {
    id: "wsl",
    name: "Wsl",
    host: "100.74.103.17",
    port: 22,
    username: "sang9604",
  },
  {
    id: "local",
    name: "Local terminal",
    host: "localhost",
    port: 0,
    username: "local",
    isLocal: true,
  },
];

/** The form now loads profiles on mount, so assertions cannot rely on call
 * order; find the call by command name instead. */
function callFor(command: string) {
  const call = mockInvoke.mock.calls.find(([name]) => name === command);
  if (!call) {
    throw new Error(
      `no ${command} invoke, saw: ${mockInvoke.mock.calls.map(([n]) => n).join(", ")}`,
    );
  }
  return call[1] as Record<string, unknown>;
}

const existing: LlmInstance = {
  id: "inst-1",
  name: "GPU server vLLM",
  runtimeType: "vllm",
  baseUrl: "http://192.168.0.21:8000",
  enabled: true,
  requestTimeoutMs: 3000,
  pollIntervalSecs: 5,
  createdAt: 1,
  updatedAt: 1,
};

describe("LlmInstanceForm", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockConfirm.mockReset();
    mockInvoke.mockImplementation((command) =>
      command === "load_sessions"
        ? Promise.resolve(profiles)
        : Promise.resolve([]),
    );
  });

  it("rejects an address that is not http or https before calling the backend", async () => {
    render(
      <LlmInstanceForm
        editing={null}
        hasApiKey={false}
        onSaved={() => undefined}
        onCancel={() => undefined}
      />,
    );

    fireEvent.change(screen.getByLabelText("Address"), {
      target: { value: "ftp://host:21" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add" }));

    expect(
      await screen.findByText("Address must start with http:// or https://"),
    ).toBeInTheDocument();
    expect(
      mockInvoke.mock.calls.some(([name]) => name === "save_llm_instance"),
    ).toBe(false);
  });

  it("requires an address", async () => {
    render(
      <LlmInstanceForm
        editing={null}
        hasApiKey={false}
        onSaved={() => undefined}
        onCancel={() => undefined}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Add" }));

    expect(await screen.findByText("Address is required")).toBeInTheDocument();
    expect(
      mockInvoke.mock.calls.some(([name]) => name === "save_llm_instance"),
    ).toBe(false);
  });

  it("sends the API key as a separate argument and drops it from state after saving", async () => {
    const onSaved = vi.fn();
    render(
      <LlmInstanceForm
        editing={null}
        hasApiKey={false}
        onSaved={onSaved}
        onCancel={() => undefined}
      />,
    );

    fireEvent.change(screen.getByLabelText("Address"), {
      target: { value: "http://192.168.0.21:8000" },
    });
    const keyField = screen.getByLabelText("API key");
    fireEvent.change(keyField, { target: { value: "sk-secret" } });
    fireEvent.click(screen.getByRole("button", { name: "Add" }));

    await waitFor(() => expect(onSaved).toHaveBeenCalled());
    const args = callFor("save_llm_instance") as {
      instance: LlmInstance;
      apiKey?: string;
    };
    expect(args.apiKey).toBe("sk-secret");
    // The key is not a field on the instance that gets persisted.
    expect(args.instance).not.toHaveProperty("apiKey");
    // Nor does it linger in the form once it has been handed over.
    expect(keyField).toHaveValue("");
  });

  it("leaves a stored key untouched when the field is not edited", async () => {
    render(
      <LlmInstanceForm
        editing={existing}
        hasApiKey
        onSaved={() => undefined}
        onCancel={() => undefined}
      />,
    );

    expect(screen.getByPlaceholderText("Stored — leave blank to keep")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(
        mockInvoke.mock.calls.some(([name]) => name === "save_llm_instance"),
      ).toBe(true),
    );
    // `undefined` means "keep what is stored"; an empty string would clear it.
    expect(callFor("save_llm_instance").apiKey).toBeUndefined();
  });

  it("clears a stored key on request", async () => {
    render(
      <LlmInstanceForm
        editing={existing}
        hasApiKey
        onSaved={() => undefined}
        onCancel={() => undefined}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Remove stored key" }));
    expect(
      screen.getByText("The stored key will be removed when you save."),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(
        mockInvoke.mock.calls.some(([name]) => name === "save_llm_instance"),
      ).toBe(true),
    );
    expect(callFor("save_llm_instance").apiKey).toBe("");
  });

  it("offers saved SSH profiles but never a local terminal", async () => {
    render(
      <LlmInstanceForm
        editing={null}
        hasApiKey={false}
        onSaved={() => undefined}
        onCancel={() => undefined}
      />,
    );

    const select = await screen.findByLabelText("Reach through");
    expect(select).toHaveValue("");
    expect(
      await screen.findByRole("option", { name: /Wsl \(sang9604@100\.74\.103\.17\)/ }),
    ).toBeInTheDocument();
    // A local terminal has no SSH transport to tunnel through.
    expect(
      screen.queryByRole("option", { name: /Local terminal/ }),
    ).not.toBeInTheDocument();
  });

  it("sends the chosen profile and explains what the address now means", async () => {
    const onSaved = vi.fn();
    render(
      <LlmInstanceForm
        editing={null}
        hasApiKey={false}
        onSaved={onSaved}
        onCancel={() => undefined}
      />,
    );

    fireEvent.change(await screen.findByLabelText("Reach through"), {
      target: { value: "wsl" },
    });
    // The caption has to say whose loopback 127.0.0.1 is.
    expect(
      screen.getByText(/resolved on 100\.74\.103\.17/),
    ).toBeInTheDocument();
    expect(
      screen.getByPlaceholderText("http://127.0.0.1:11434"),
    ).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Address"), {
      target: { value: "http://127.0.0.1:11434" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add" }));

    await waitFor(() => expect(onSaved).toHaveBeenCalled());
    const args = callFor("save_llm_instance") as { instance: LlmInstance };
    expect(args.instance.sshProfileId).toBe("wsl");
    expect(args.instance.baseUrl).toBe("http://127.0.0.1:11434");
  });

  it("warns when a loopback address is polled directly", async () => {
    render(
      <LlmInstanceForm
        editing={null}
        hasApiKey={false}
        onSaved={() => undefined}
        onCancel={() => undefined}
      />,
    );

    await screen.findByLabelText("Reach through");
    fireEvent.change(screen.getByLabelText("Address"), {
      target: { value: "http://127.0.0.1:11434" },
    });

    // The failure mode this prevents is a bare "connection refused" that says
    // nothing about the runtime actually being on another host.
    expect(
      screen.getByText(/Reach through is set to Direct/),
    ).toBeInTheDocument();

    // Choosing the hop resolves it, and the caption changes to say whose it is.
    fireEvent.change(screen.getByLabelText("Reach through"), {
      target: { value: "wsl" },
    });
    expect(
      screen.queryByText(/Reach through is set to Direct/),
    ).not.toBeInTheDocument();
    expect(screen.getByText(/resolved on 100\.74\.103\.17/)).toBeInTheDocument();
  });

  it("does not warn about a non-loopback address polled directly", async () => {
    render(
      <LlmInstanceForm
        editing={null}
        hasApiKey={false}
        onSaved={() => undefined}
        onCancel={() => undefined}
      />,
    );

    await screen.findByLabelText("Reach through");
    for (const address of [
      "http://192.168.0.20:11434",
      "http://100.74.103.17:11434",
      "http://ollama.internal:11434",
    ]) {
      fireEvent.change(screen.getByLabelText("Address"), {
        target: { value: address },
      });
      expect(
        screen.queryByText(/Reach through is set to Direct/),
        address,
      ).not.toBeInTheDocument();
    }

    // localhost and an IPv6 loopback are the same mistake and must warn.
    for (const address of ["http://localhost:11434", "http://[::1]:11434"]) {
      fireEvent.change(screen.getByLabelText("Address"), {
        target: { value: address },
      });
      expect(screen.getByText(/Reach through is set to Direct/)).toBeInTheDocument();
    }
  });

  it("blocks https over a tunnel before making a round trip", async () => {
    render(
      <LlmInstanceForm
        editing={null}
        hasApiKey={false}
        onSaved={() => undefined}
        onCancel={() => undefined}
      />,
    );

    fireEvent.change(await screen.findByLabelText("Reach through"), {
      target: { value: "wsl" },
    });
    fireEvent.change(screen.getByLabelText("Address"), {
      target: { value: "https://127.0.0.1:11434" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add" }));

    // The certificate would be validated against 127.0.0.1 and could not match.
    expect(
      await screen.findByText(/checked against 127\.0\.0\.1/),
    ).toBeInTheDocument();
    expect(
      mockInvoke.mock.calls.some(([name]) => name === "save_llm_instance"),
    ).toBe(false);

    // Without a tunnel the same address is fine.
    fireEvent.change(screen.getByLabelText("Reach through"), {
      target: { value: "" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add" }));
    await waitFor(() =>
      expect(
        mockInvoke.mock.calls.some(([name]) => name === "save_llm_instance"),
      ).toBe(true),
    );
  });

  it("bootstraps host-key trust from the connection test and retries", async () => {
    let attempts = 0;
    mockInvoke.mockImplementation((command) => {
      if (command === "load_sessions") {
        return Promise.resolve(profiles);
      }
      if (command === "test_llm_instance") {
        attempts += 1;
        if (attempts === 1) {
          // The one path that can show a fingerprint prompt: the poller cannot.
          return Promise.reject(
            "UNKNOWN_HOST_KEY:abc123|ssh-ed25519|100.74.103.17:22",
          );
        }
        return Promise.resolve({
          instanceId: "inst-1",
          runtimeType: "ollama",
          status: "online",
          responseTimeMs: 8,
          checkedAt: 1,
        });
      }
      return Promise.resolve([]);
    });
    mockConfirm.mockResolvedValue(true);

    render(
      <LlmInstanceForm
        editing={null}
        hasApiKey={false}
        onSaved={() => undefined}
        onCancel={() => undefined}
      />,
    );

    fireEvent.change(await screen.findByLabelText("Reach through"), {
      target: { value: "wsl" },
    });
    fireEvent.change(screen.getByLabelText("Address"), {
      target: { value: "http://127.0.0.1:11434" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Test connection" }));

    expect(await screen.findByText(/Reachable in 8 ms/)).toBeInTheDocument();
    expect(mockConfirm).toHaveBeenCalledTimes(1);
    expect(callFor("trust_host_key")).toMatchObject({
      host: "100.74.103.17",
      port: 22,
      keyType: "ssh-ed25519",
      fingerprint: "abc123",
    });
    expect(attempts).toBe(2);
  });

  it("reports the result of a connection test without saving", async () => {
    mockInvoke.mockImplementation((command) =>
      command === "load_sessions"
        ? Promise.resolve(profiles)
        : Promise.resolve({
            instanceId: "inst-1",
            runtimeType: "vllm",
            status: "offline",
            responseTimeMs: null,
            checkedAt: 1,
            errorCode: "connection_refused",
            errorMessage: "The server refused the connection.",
          }),
    );

    render(
      <LlmInstanceForm
        editing={existing}
        hasApiKey={false}
        onSaved={() => undefined}
        onCancel={() => undefined}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Test connection" }));

    expect(
      await screen.findByText(/offline: The server refused the connection\./),
    ).toBeInTheDocument();
    expect(
      mockInvoke.mock.calls.some(([name]) => name === "save_llm_instance"),
    ).toBe(false);
    expect(callFor("test_llm_instance")).toBeDefined();
  });
});
