import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { check, type DownloadEvent, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { SKIPPED_UPDATE_KEY, useUpdateStore } from "./updateStore";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: vi.fn() }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));

const mockInvoke = vi.mocked(invoke);
const mockGetVersion = vi.mocked(getVersion);
const mockCheck = vi.mocked(check);
const mockRelaunch = vi.mocked(relaunch);

function fakeUpdate(
  version: string,
  downloadAndInstall?: (cb?: (event: DownloadEvent) => void) => Promise<void>,
): Update {
  return {
    version,
    currentVersion: "1.0.0",
    body: "Release notes",
    downloadAndInstall: vi.fn(downloadAndInstall ?? (async () => undefined)),
  } as unknown as Update;
}

describe("updateStore", () => {
  const storage = new Map<string, string>();

  beforeEach(() => {
    storage.clear();
    vi.stubGlobal("localStorage", {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, value),
      removeItem: (key: string) => storage.delete(key),
    });
    mockInvoke.mockReset();
    mockGetVersion.mockReset();
    mockCheck.mockReset();
    mockRelaunch.mockReset();
    useUpdateStore.setState({
      currentVersion: null,
      installMode: "in_app",
      phase: "idle",
      update: null,
      progress: null,
      error: null,
      dialogOpen: false,
      postponedVersion: null,
    });
  });

  it("initializes current version and install mode with safe fallbacks", async () => {
    mockGetVersion.mockResolvedValueOnce("1.2.3");
    mockInvoke.mockResolvedValueOnce("manual");

    await useUpdateStore.getState().init();
    expect(useUpdateStore.getState().currentVersion).toBe("1.2.3");
    expect(useUpdateStore.getState().installMode).toBe("manual");

    mockGetVersion.mockRejectedValueOnce(new Error("unavailable"));
    mockInvoke.mockRejectedValueOnce(new Error("unavailable"));
    await useUpdateStore.getState().init();
    expect(useUpdateStore.getState().currentVersion).toBeNull();
    expect(useUpdateStore.getState().installMode).toBe("in_app");
  });

  it("handles up-to-date, available, skipped, and postponed update checks", async () => {
    mockCheck.mockResolvedValueOnce(null);
    await useUpdateStore.getState().checkForUpdates(true);
    expect(useUpdateStore.getState().phase).toBe("upToDate");
    expect(useUpdateStore.getState().dialogOpen).toBe(true);

    // Automatic check opens dialog for a new version.
    useUpdateStore.setState({ dialogOpen: false });
    const update = fakeUpdate("2.0.0");
    mockCheck.mockResolvedValueOnce(update);
    await useUpdateStore.getState().checkForUpdates(false);
    expect(useUpdateStore.getState().phase).toBe("available");
    expect(useUpdateStore.getState().dialogOpen).toBe(true);

    // Closing the dialog postpones the version so another automatic check stays quiet.
    useUpdateStore.getState().closeDialog();
    expect(useUpdateStore.getState().dialogOpen).toBe(false);
    expect(useUpdateStore.getState().postponedVersion).toBe("2.0.0");

    mockCheck.mockResolvedValueOnce(update);
    await useUpdateStore.getState().checkForUpdates(false);
    expect(useUpdateStore.getState().dialogOpen).toBe(false);

    // Skipping persists to localStorage and keeps automatic checks quiet.
    useUpdateStore.getState().skipVersion();
    expect(storage.get(SKIPPED_UPDATE_KEY)).toBe("2.0.0");
    useUpdateStore.setState({ postponedVersion: null });
    mockCheck.mockResolvedValueOnce(update);
    await useUpdateStore.getState().checkForUpdates(false);
    expect(useUpdateStore.getState().dialogOpen).toBe(false);

    // Manual check still opens the dialog even for a skipped version.
    mockCheck.mockResolvedValueOnce(update);
    await useUpdateStore.getState().checkForUpdates(true);
    expect(useUpdateStore.getState().dialogOpen).toBe(true);
  });

  it("suppresses automatic check errors but reports manual check errors", async () => {
    mockCheck.mockRejectedValueOnce(new Error("offline"));
    await useUpdateStore.getState().checkForUpdates(false);
    expect(useUpdateStore.getState().phase).toBe("idle");
    expect(useUpdateStore.getState().error).toBeNull();

    mockCheck.mockRejectedValueOnce(new Error("timeout"));
    await useUpdateStore.getState().checkForUpdates(true);
    expect(useUpdateStore.getState().phase).toBe("error");
    expect(useUpdateStore.getState().error).toContain("timeout");
  });

  it("tracks download progress and transitions to ready or error on install", async () => {
    const progressValues: Array<number | null> = [];
    const unsubscribe = useUpdateStore.subscribe((state) => {
      if (state.phase === "downloading") {
        progressValues.push(state.progress);
      }
    });
    const update = fakeUpdate("2.1.0", async (cb) => {
      cb?.({ event: "Started", data: { contentLength: 200 } });
      cb?.({ event: "Progress", data: { chunkLength: 50 } });
      cb?.({ event: "Progress", data: { chunkLength: 150 } });
      cb?.({ event: "Finished" });
    });
    useUpdateStore.setState({ update, phase: "available" });

    await useUpdateStore.getState().install();
    unsubscribe();
    expect(progressValues).toEqual([null, 0, 25, 100, 100]);
    expect(useUpdateStore.getState().phase).toBe("ready");

    // While ready, another manual check only reopens the dialog without calling check().
    useUpdateStore.setState({ dialogOpen: false });
    await useUpdateStore.getState().checkForUpdates(true);
    expect(mockCheck).not.toHaveBeenCalled();
    expect(useUpdateStore.getState().dialogOpen).toBe(true);

    // Download failure reports an error.
    const failing = fakeUpdate("2.2.0", async () => {
      throw new Error("checksum mismatch");
    });
    useUpdateStore.setState({ update: failing, phase: "available" });
    await useUpdateStore.getState().install();
    expect(useUpdateStore.getState().phase).toBe("error");
    expect(useUpdateStore.getState().error).toContain("checksum mismatch");
  });

  it("relaunches and opens the release page", async () => {
    mockRelaunch.mockResolvedValueOnce(undefined);
    await useUpdateStore.getState().restart();
    expect(mockRelaunch).toHaveBeenCalledOnce();

    useUpdateStore.setState({ update: fakeUpdate("2.3.0") });
    mockInvoke.mockResolvedValueOnce(undefined);
    await useUpdateStore.getState().openDownloadPage();
    expect(mockInvoke).toHaveBeenCalledWith("open_release_page", { version: "2.3.0" });
  });
});
