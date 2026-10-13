import { act, renderHook } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useSessionStore } from "../stores/sessionStore";
import {
  isPermanentReconnectError,
  reconnectDelayMs,
  useAutoReconnect,
} from "./useAutoReconnect";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

const profile = {
  id: "gpu",
  name: "gpu box",
  host: "gpu.lab",
  port: 22,
  username: "me",
};

describe("useAutoReconnect", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    mockInvoke.mockReset();
    useSessionStore.setState({
      sessions: [profile],
      activeSessionId: null,
      connectedSessionIds: [],
      terminalIdsBySession: {},
      lostSessionIds: [],
      message: null,
    });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("reconnects a lost session after the first backoff step", async () => {
    mockInvoke.mockResolvedValue({
      sessionId: "gpu",
      terminalId: "t-2",
      profile,
    });
    renderHook(() => useAutoReconnect());
    act(() => useSessionStore.getState().markSessionLost("gpu"));

    await act(async () => {
      await vi.advanceTimersByTimeAsync(reconnectDelayMs(0));
    });

    expect(mockInvoke).toHaveBeenCalledWith("connect_terminal", {
      request: expect.objectContaining({
        id: "gpu",
        reuseStoredCredentials: true,
        // An automatic reconnect must never pop up a sign-in dialog.
        allowInteractiveAuth: false,
      }),
    });
    const state = useSessionStore.getState();
    expect(state.connectedSessionIds).toEqual(["gpu"]);
    expect(state.lostSessionIds).toEqual([]);
    expect(state.activeSessionId).toBe("gpu");
  });

  it("stops on errors a retry cannot fix", async () => {
    mockInvoke.mockRejectedValue("HOST_KEY_CHANGED:aa|bb|ssh-ed25519|gpu.lab:22");
    renderHook(() => useAutoReconnect());
    act(() => useSessionStore.getState().markSessionLost("gpu"));

    await act(async () => {
      await vi.advanceTimersByTimeAsync(reconnectDelayMs(0));
    });

    expect(useSessionStore.getState().lostSessionIds).toEqual([]);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(120_000);
    });
    expect(mockInvoke).toHaveBeenCalledTimes(1);
  });

  it("does not retry a session the user disconnected", async () => {
    renderHook(() => useAutoReconnect());
    act(() => {
      useSessionStore.getState().markSessionLost("gpu");
      useSessionStore.getState().clearSessionLost("gpu");
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(120_000);
    });
    expect(mockInvoke).not.toHaveBeenCalled();
  });
});

describe("reconnect policy", () => {
  it("backs off and classifies permanent errors", () => {
    expect(reconnectDelayMs(0)).toBe(2000);
    expect(reconnectDelayMs(99)).toBe(60_000);
    expect(isPermanentReconnectError("SSH authentication failed")).toBe(true);
    expect(isPermanentReconnectError("Network timeout")).toBe(false);
    expect(isPermanentReconnectError("INTERACTIVE_AUTH_REQUIRED:alice@gpu.lab:22")).toBe(true);
  });
});
