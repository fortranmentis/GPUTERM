import { act, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { DetailWindow } from "./DetailWindow";
import { LLM_INSTANCES_CHANGED_EVENT } from "../hooks/useLlmInstancesSync";
import type { LlmInstance } from "../types/llm";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ close: vi.fn() }),
}));

const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);

function instance(id: string, name: string): LlmInstance {
  return {
    id,
    name,
    runtimeType: "ollama",
    baseUrl: "http://192.168.0.20:11434",
    enabled: true,
    requestTimeoutMs: 3000,
    pollIntervalSecs: 5,
    createdAt: 1,
    updatedAt: 1,
  };
}

describe("DetailWindow LLM window", () => {
  const handlers = new Map<string, (event: { payload: unknown }) => void>();

  beforeEach(() => {
    handlers.clear();
    window.history.replaceState(null, "", "/?window=detail&resource=llm");
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async (command) => {
      if (command === "list_llm_instances") {
        return [instance("a", "First server")];
      }
      if (command === "get_telemetry_settings") {
        return {
          telemetryIntervalSecs: 2,
          displayMode: "gpu-system",
          diskIgnoreFsTypes: [],
        };
      }
      return null;
    });
    mockListen.mockReset();
    mockListen.mockImplementation(async (event, handler) => {
      handlers.set(event, handler as (event: { payload: unknown }) => void);
      return () => undefined;
    });
  });

  it("shows instances added in the main window", async () => {
    render(<DetailWindow />);
    expect((await screen.findAllByText("First server")).length).toBeGreaterThan(0);
    await waitFor(() => expect(handlers.has(LLM_INSTANCES_CHANGED_EVENT)).toBe(true));

    act(() =>
      handlers.get(LLM_INSTANCES_CHANGED_EVENT)!({
        payload: [instance("a", "First server"), instance("b", "Second server")],
      }),
    );

    expect((await screen.findAllByText("Second server")).length).toBeGreaterThan(0);
  });
});
