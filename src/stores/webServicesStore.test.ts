import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  WEB_SERVICE_NOTIFY_KEY,
  useWebServicesStore,
} from "./webServicesStore";
import type { WebService, WebServiceScan } from "../types/webServices";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

function service(port: number, kind: WebService["kind"] = "tensorboard"): WebService {
  return {
    kind,
    pid: 1000 + port,
    host: "localhost",
    port,
    path: "/",
    detail: null,
    container: null,
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

describe("useWebServicesStore", () => {
  const storage = new Map<string, string>();

  beforeEach(() => {
    mockInvoke.mockReset();
    storage.clear();
    vi.stubGlobal("localStorage", {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => {
        storage.set(key, value);
      },
      removeItem: (key: string) => {
        storage.delete(key);
      },
    });
    useWebServicesStore.setState({
      scans: {},
      errors: {},
      scanning: {},
      notify: true,
    });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("scans services and reports only newly discovered ones on subsequent scans", async () => {
    const tb = service(6006, "tensorboard");
    const jupyter = service(8888, "jupyter");

    mockInvoke.mockResolvedValueOnce({ supported: true, services: [tb] });
    const first = await useWebServicesStore.getState().refresh("s1");
    expect(first).toEqual([]);
    expect(useWebServicesStore.getState().scans.s1).toEqual({
      supported: true,
      services: [tb],
    });
    expect(useWebServicesStore.getState().scanning.s1).toBe(false);

    mockInvoke.mockResolvedValueOnce({ supported: true, services: [tb, jupyter] });
    const second = await useWebServicesStore.getState().refresh("s1");
    expect(second).toEqual([jupyter]);
    expect(useWebServicesStore.getState().scans.s1?.services).toEqual([tb, jupyter]);
  });

  it("records errors on failure and clears them on the next successful scan", async () => {
    mockInvoke.mockRejectedValueOnce(new Error("SSH channel closed"));
    const failed = await useWebServicesStore.getState().refresh("s1");
    expect(failed).toEqual([]);
    expect(useWebServicesStore.getState().errors.s1).toContain("SSH channel closed");
    expect(useWebServicesStore.getState().scanning.s1).toBe(false);

    mockInvoke.mockResolvedValueOnce({ supported: true, services: [] });
    await useWebServicesStore.getState().refresh("s1");
    expect(useWebServicesStore.getState().errors.s1).toBeUndefined();
    expect(useWebServicesStore.getState().scans.s1).toEqual({
      supported: true,
      services: [],
    });
  });

  it("prevents stale in-flight scan results from repopulating after forget(sessionId)", async () => {
    const inflight = deferred<WebServiceScan>();
    mockInvoke.mockReturnValueOnce(inflight.promise);

    const refreshPromise = useWebServicesStore.getState().refresh("s1");
    expect(useWebServicesStore.getState().scanning.s1).toBe(true);

    // Session disconnects while scan is still in flight.
    useWebServicesStore.getState().forget("s1");
    expect(useWebServicesStore.getState().scans.s1).toBeUndefined();
    expect(useWebServicesStore.getState().scanning.s1).toBeUndefined();

    // Stale response resolves after disconnect.
    inflight.resolve({ supported: true, services: [service(6006)] });
    const discovered = await refreshPromise;

    expect(discovered).toEqual([]);
    expect(useWebServicesStore.getState().scans.s1).toBeUndefined();
    expect(useWebServicesStore.getState().errors.s1).toBeUndefined();
    expect(useWebServicesStore.getState().scanning.s1).toBeUndefined();
  });

  it("ignores stale rejections after forget(sessionId) and does not disrupt a reconnected scan", async () => {
    const stale = deferred<WebServiceScan>();
    const fresh = deferred<WebServiceScan>();
    mockInvoke.mockReturnValueOnce(stale.promise).mockReturnValueOnce(fresh.promise);

    const stalePromise = useWebServicesStore.getState().refresh("s1");
    useWebServicesStore.getState().forget("s1");

    // Reconnect and start a fresh scan before the stale one rejects.
    const freshPromise = useWebServicesStore.getState().refresh("s1");
    expect(useWebServicesStore.getState().scanning.s1).toBe(true);

    stale.reject(new Error("old connection dropped"));
    await stalePromise;

    // Fresh scan must remain in flight with no stale error written.
    expect(useWebServicesStore.getState().errors.s1).toBeUndefined();
    expect(useWebServicesStore.getState().scanning.s1).toBe(true);

    const ray = service(8265, "ray");
    fresh.resolve({ supported: true, services: [ray] });
    await freshPromise;

    expect(useWebServicesStore.getState().scans.s1).toEqual({
      supported: true,
      services: [ray],
    });
    expect(useWebServicesStore.getState().scanning.s1).toBe(false);
  });

  it("persists notification preference to localStorage", () => {
    useWebServicesStore.getState().setNotify(false);
    expect(useWebServicesStore.getState().notify).toBe(false);
    expect(localStorage.getItem(WEB_SERVICE_NOTIFY_KEY)).toBe("off");

    useWebServicesStore.getState().setNotify(true);
    expect(useWebServicesStore.getState().notify).toBe(true);
    expect(localStorage.getItem(WEB_SERVICE_NOTIFY_KEY)).toBe("on");
  });
});
