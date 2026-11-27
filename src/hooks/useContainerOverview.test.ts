import { act, renderHook } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  CONTAINER_CARD_REFRESH_MS,
  CONTAINER_POPOVER_REFRESH_MS,
  useContainerOverview,
} from "./useContainerOverview";
import type { ContainerOverview } from "../types/containers";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

function overview(runtime: string): ContainerOverview {
  return {
    runtime,
    containers: [],
    kubectl: null,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("useContainerOverview", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    mockInvoke.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("fetches the new host even when the previous host's fetch is still in flight", async () => {
    const hostA = deferred<ContainerOverview>();
    const hostB = deferred<ContainerOverview>();
    mockInvoke.mockImplementation((_cmd, args) => {
      const { sessionId } = args as { sessionId: string };
      return sessionId === "host-a" ? hostA.promise : hostB.promise;
    });

    const { result, rerender } = renderHook(
      ({ sessionId }: { sessionId: string | null }) =>
        useContainerOverview(sessionId, true, false),
      { initialProps: { sessionId: "host-a" } },
    );

    expect(mockInvoke).toHaveBeenCalledTimes(1);
    expect(mockInvoke).toHaveBeenCalledWith("container_overview", {
      sessionId: "host-a",
    });
    expect(result.current.loading).toBe(true);

    // Switch to host B while host A's request is still pending.
    rerender({ sessionId: "host-b" });

    expect(mockInvoke).toHaveBeenCalledTimes(2);
    expect(mockInvoke).toHaveBeenLastCalledWith("container_overview", {
      sessionId: "host-b",
    });

    // Host B resolves first and updates state.
    await act(async () => {
      hostB.resolve(overview("docker"));
      await hostB.promise;
    });
    expect(result.current.overview).toEqual(overview("docker"));
    expect(result.current.loading).toBe(false);

    // Slow host A resolves later and must not overwrite host B's overview.
    await act(async () => {
      hostA.resolve(overview("podman"));
      await hostA.promise;
    });
    expect(result.current.overview).toEqual(overview("docker"));
  });

  it("marks non-Linux hosts unsupported and stops polling them", async () => {
    mockInvoke.mockRejectedValueOnce("Linux hosts only");

    const { result } = renderHook(() =>
      useContainerOverview("macos-host", true, false),
    );

    await act(async () => {
      await Promise.resolve();
    });

    expect(result.current.unsupported).toBe(true);
    expect(result.current.error).toContain("Linux hosts only");
    expect(mockInvoke).toHaveBeenCalledTimes(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(CONTAINER_CARD_REFRESH_MS * 2);
    });
    expect(mockInvoke).toHaveBeenCalledTimes(1);
  });

  it("polls faster when detailed and skips an extra immediate read on close", async () => {
    mockInvoke.mockResolvedValue(overview("docker"));

    const { rerender } = renderHook(
      ({ detailed }: { detailed: boolean }) =>
        useContainerOverview("host-a", true, detailed),
      { initialProps: { detailed: false } },
    );

    await act(async () => {
      await Promise.resolve();
    });
    expect(mockInvoke).toHaveBeenCalledTimes(1);

    // Opening detailed view triggers an immediate fetch and polls every 15s.
    rerender({ detailed: true });
    await act(async () => {
      await Promise.resolve();
    });
    expect(mockInvoke).toHaveBeenCalledTimes(2);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(CONTAINER_POPOVER_REFRESH_MS);
    });
    expect(mockInvoke).toHaveBeenCalledTimes(3);

    // Closing detailed view does not trigger an immediate fetch.
    rerender({ detailed: false });
    await act(async () => {
      await Promise.resolve();
    });
    expect(mockInvoke).toHaveBeenCalledTimes(3);
  });
});
