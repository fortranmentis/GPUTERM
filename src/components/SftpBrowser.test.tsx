import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { SftpBrowser } from "./SftpBrowser";
import { useSessionStore } from "../stores/sessionStore";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);
const mockOpen = vi.mocked(open);

describe("SftpBrowser local path browse", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockOpen.mockReset();
    useSessionStore.setState({
      activeSessionId: "session-1",
      connected: true,
      message: null,
    });
    mockInvoke.mockImplementation((command, args) => {
      if (command === "load_app_settings") {
        return Promise.resolve({ recentLocalPath: "C:\\Users\\me" });
      }
      if (command === "list_local_dir") {
        return Promise.resolve({
          path: (args as { path: string }).path,
          entries: [],
        });
      }
      if (command === "update_recent_local_path") {
        return Promise.resolve({
          recentLocalPath: (args as { path: string }).path,
        });
      }
      if (command === "sftp_list_dir") {
        return Promise.resolve({ path: ".", entries: [] });
      }
      return Promise.resolve(undefined);
    });
  });

  it("calls dialog when Browse is clicked", async () => {
    mockOpen.mockResolvedValue(null);
    render(<SftpBrowser />);

    fireEvent.click(screen.getByRole("button", { name: /browse/i }));

    await waitFor(() => expect(mockOpen).toHaveBeenCalled());
  });

  it("updates local path after selecting a folder", async () => {
    mockOpen.mockResolvedValue("C:\\Users\\me\\Downloads");
    render(<SftpBrowser />);

    fireEvent.click(screen.getByRole("button", { name: /browse/i }));

    await waitFor(() =>
      expect(screen.getByLabelText(/local path/i)).toHaveValue(
        "C:\\Users\\me\\Downloads",
      ),
    );
    expect(mockInvoke).toHaveBeenCalledWith("update_recent_local_path", {
      path: "C:\\Users\\me\\Downloads",
    });
    expect(mockInvoke).toHaveBeenCalledWith("list_local_dir", {
      path: "C:\\Users\\me\\Downloads",
    });
  });

  it("keeps local path when folder selection is cancelled", async () => {
    mockOpen.mockResolvedValue(null);
    render(<SftpBrowser />);

    await waitFor(() =>
      expect(screen.getByLabelText(/local path/i)).toHaveValue("C:\\Users\\me"),
    );
    fireEvent.click(screen.getByRole("button", { name: /browse/i }));

    await waitFor(() => expect(mockOpen).toHaveBeenCalled());
    expect(screen.getByLabelText(/local path/i)).toHaveValue("C:\\Users\\me");
  });
});
