import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
const check = vi.fn();
const relaunch = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: () => Promise.resolve("1.3.7-beta") }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: () => check() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: () => relaunch() }));

import { UpdateDialog } from "./UpdateDialog";
import { SidebarFooter } from "./SidebarFooter";
import { SKIPPED_UPDATE_KEY, useUpdateStore } from "../stores/updateStore";

type ProgressEvent =
  | { event: "Started"; data: { contentLength?: number } }
  | { event: "Progress"; data: { chunkLength: number } }
  | { event: "Finished" };

function fakeUpdate(version = "1.3.8-beta") {
  return {
    version,
    body: "Faster telemetry",
    downloadAndInstall: vi.fn(async (onEvent: (event: ProgressEvent) => void) => {
      onEvent({ event: "Started", data: { contentLength: 100 } });
      onEvent({ event: "Progress", data: { chunkLength: 100 } });
      onEvent({ event: "Finished" });
    }),
  };
}

const initialState = useUpdateStore.getState();

// Node's own (disabled) localStorage shadows jsdom's, so give the store one.
const storage = new Map<string, string>();
vi.stubGlobal("localStorage", {
  getItem: (key: string) => storage.get(key) ?? null,
  setItem: (key: string, value: string) => void storage.set(key, String(value)),
  removeItem: (key: string) => void storage.delete(key),
  clear: () => storage.clear(),
});

beforeEach(() => {
  invoke.mockReset();
  check.mockReset();
  relaunch.mockReset();
  localStorage.clear();
  useUpdateStore.setState(initialState, true);
  invoke.mockImplementation((command: string) =>
    Promise.resolve(command === "update_install_mode" ? "in_app" : undefined),
  );
});

async function renderWithVersion() {
  render(
    <>
      <SidebarFooter />
      <UpdateDialog />
    </>,
  );
  await act(() => useUpdateStore.getState().init());
}

describe("UpdateDialog", () => {
  it("offers a new version found by an automatic check and installs it", async () => {
    const update = fakeUpdate();
    check.mockResolvedValue(update);
    await renderWithVersion();

    await act(() => useUpdateStore.getState().checkForUpdates(false));

    expect(screen.getByText("GpuTerm 1.3.8-beta is available")).toBeInTheDocument();
    expect(screen.getByText("You have 1.3.7-beta.")).toBeInTheDocument();
    expect(screen.getByLabelText("Release notes")).toHaveTextContent("Faster telemetry");

    fireEvent.click(screen.getByRole("button", { name: "Update" }));
    await screen.findByText("The update is installed. Restart GpuTerm to use it.");
    expect(update.downloadAndInstall).toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Restart now" }));
    await waitFor(() => expect(relaunch).toHaveBeenCalled());
  });

  it("stays quiet about a skipped version until the user checks by hand", async () => {
    check.mockResolvedValue(fakeUpdate());
    await renderWithVersion();
    await act(() => useUpdateStore.getState().checkForUpdates(false));

    fireEvent.click(screen.getByRole("button", { name: "Skip this version" }));
    expect(localStorage.getItem(SKIPPED_UPDATE_KEY)).toBe("1.3.8-beta");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    await act(() => useUpdateStore.getState().checkForUpdates(false));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    // The footer still points at it.
    expect(screen.getByRole("button", { name: /Update to 1\.3\.8-beta/ })).toBeInTheDocument();

    await act(() => useUpdateStore.getState().checkForUpdates(true));
    expect(screen.getByRole("dialog", { name: "Software update" })).toBeInTheDocument();
  });

  it("does not reopen after Later on the next automatic check", async () => {
    check.mockResolvedValue(fakeUpdate());
    await renderWithVersion();
    await act(() => useUpdateStore.getState().checkForUpdates(false));
    fireEvent.click(screen.getByRole("button", { name: "Later" }));

    await act(() => useUpdateStore.getState().checkForUpdates(false));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("opens the release page instead of installing a package-managed build", async () => {
    invoke.mockImplementation((command: string) =>
      Promise.resolve(command === "update_install_mode" ? "manual" : undefined),
    );
    check.mockResolvedValue(fakeUpdate());
    await renderWithVersion();
    await act(() => useUpdateStore.getState().checkForUpdates(false));

    expect(screen.queryByRole("button", { name: "Update" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Open download page" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("open_release_page", { version: "1.3.8-beta" }),
    );
  });

  it("reports a failed manual check but not a failed automatic one", async () => {
    check.mockRejectedValue(new Error("offline"));
    await renderWithVersion();

    await act(() => useUpdateStore.getState().checkForUpdates(false));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("offline");
  });

  it("says so when there is nothing newer", async () => {
    check.mockResolvedValue(null);
    await renderWithVersion();
    expect(screen.getByText("v1.3.7-beta")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    expect(await screen.findByText("GpuTerm is up to date")).toBeInTheDocument();
    expect(screen.getByText("You have the latest version (1.3.7-beta).")).toBeInTheDocument();
  });
});
