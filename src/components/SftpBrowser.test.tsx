import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { confirm, open } from "@tauri-apps/plugin-dialog";
import { SftpBrowser } from "./SftpBrowser";
import { useSessionStore } from "../stores/sessionStore";
import { useTransferStore } from "../stores/transferStore";
import type { LocalEntry, SftpEntry } from "../types/session";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  confirm: vi.fn(),
  open: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);
const mockOpen = vi.mocked(open);
const mockConfirm = vi.mocked(confirm);
const LOCAL_DRAG_TYPE = "application/x-gputerm-local-files";
const REMOTE_DRAG_TYPE = "application/x-gputerm-remote-files";

function localFile(name: string): LocalEntry {
  return {
    name,
    path: `C:\\Users\\me\\${name}`,
    entryType: "file",
    size: 10,
    modifiedTime: null,
  };
}

function remoteFile(name: string): SftpEntry {
  return {
    name,
    path: `/srv/${name}`,
    type: "file",
    size: 20,
    modifiedTime: null,
  };
}

function dragData(type: string, payload: unknown[]) {
  return {
    getData: (requestedType: string) =>
      requestedType === type ? JSON.stringify(payload) : "",
    files: [],
    effectAllowed: "copy",
    setData: vi.fn(),
  };
}

describe("SftpBrowser local path browse", () => {
  let localEntries: LocalEntry[];
  let remoteEntries: SftpEntry[];

  beforeEach(() => {
    mockInvoke.mockReset();
    mockOpen.mockReset();
    mockConfirm.mockReset();
    mockConfirm.mockResolvedValue(true);
    localEntries = [];
    remoteEntries = [];
    useTransferStore.setState({ tasks: [], activeDrag: null });
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
          entries: localEntries,
        });
      }
      if (command === "update_recent_local_path") {
        return Promise.resolve({
          recentLocalPath: (args as { path: string }).path,
        });
      }
      if (command === "sftp_list_dir") {
        return Promise.resolve({ path: "/srv", entries: remoteEntries });
      }
      if (command === "sftp_path_exists" || command === "local_path_exists") {
        return Promise.resolve(false);
      }
      if (command === "sftp_upload_file" || command === "sftp_download_file") {
        return Promise.resolve(undefined);
      }
      return Promise.resolve(undefined);
    });
  });

  it("fails the initial recent-path load silently", async () => {
    mockInvoke.mockImplementation((command) => {
      if (command === "load_app_settings") {
        return Promise.resolve({ recentLocalPath: "D:\\gone" });
      }
      if (command === "list_local_dir") {
        return Promise.reject("Local path is unavailable or does not exist");
      }
      if (command === "sftp_list_dir") {
        return Promise.resolve({ path: "/srv", entries: [] });
      }
      return Promise.resolve(undefined);
    });

    render(<SftpBrowser />);

    await waitFor(() =>
      expect(screen.getByLabelText(/local path/i)).toHaveValue("D:\\gone"),
    );
    await waitFor(() =>
      expect(
        mockInvoke.mock.calls.some(([command]) => command === "list_local_dir"),
      ).toBe(true),
    );
    expect(useSessionStore.getState().message).toBeNull();
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

  it("creates an upload task when local files are dropped on the remote panel", async () => {
    localEntries = [localFile("alpha.txt")];
    render(<SftpBrowser />);

    await waitFor(() => expect(screen.getByText("/srv")).toBeInTheDocument());
    fireEvent.drop(screen.getByTestId("remote-drop-zone"), {
      dataTransfer: dragData(LOCAL_DRAG_TYPE, [localEntries[0]]),
    });

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("sftp_upload_file", {
        request: expect.objectContaining({
          localPath: "C:\\Users\\me\\alpha.txt",
          remotePath: "/srv/alpha.txt",
        }),
      }),
    );
    expect(screen.getAllByText("alpha.txt").length).toBeGreaterThan(1);
    expect(screen.getByText("upload")).toBeInTheDocument();
  });

  it("creates a download task when remote files are dropped on the local panel", async () => {
    remoteEntries = [remoteFile("beta.log")];
    render(<SftpBrowser />);

    await waitFor(() => expect(screen.getByText("/srv")).toBeInTheDocument());
    fireEvent.drop(screen.getByTestId("local-drop-zone"), {
      dataTransfer: dragData(REMOTE_DRAG_TYPE, [remoteEntries[0]]),
    });

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("sftp_download_file", {
        request: expect.objectContaining({
          remotePath: "/srv/beta.log",
          localPath: "C:\\Users\\me\\beta.log",
        }),
      }),
    );
    expect(screen.getAllByText("beta.log").length).toBeGreaterThan(1);
    expect(screen.getByText("download")).toBeInTheDocument();
  });

  it("creates multiple transfer tasks when multiple files are dropped", async () => {
    const files = [localFile("one.txt"), localFile("two.txt")];
    render(<SftpBrowser />);

    await waitFor(() => expect(screen.getByText("/srv")).toBeInTheDocument());
    fireEvent.drop(screen.getByTestId("remote-drop-zone"), {
      dataTransfer: dragData(LOCAL_DRAG_TYPE, files),
    });

    await waitFor(() =>
      expect(
        mockInvoke.mock.calls.filter(([command]) => command === "sftp_upload_file"),
      ).toHaveLength(2),
    );
    expect(screen.getByText("one.txt")).toBeInTheDocument();
    expect(screen.getByText("two.txt")).toBeInTheDocument();
  });

  it("skips directories in a mixed drop with a single message and uploads only files", async () => {
    const directoryEntry: LocalEntry = {
      name: "logs",
      path: "C:\\Users\\me\\logs",
      entryType: "directory",
      size: null,
      modifiedTime: null,
    };
    const otherDirectoryEntry: LocalEntry = {
      ...directoryEntry,
      name: "cache",
      path: "C:\\Users\\me\\cache",
    };
    render(<SftpBrowser />);

    await waitFor(() => expect(screen.getByText("/srv")).toBeInTheDocument());
    fireEvent.drop(screen.getByTestId("remote-drop-zone"), {
      dataTransfer: dragData(LOCAL_DRAG_TYPE, [
        directoryEntry,
        localFile("kept.txt"),
        otherDirectoryEntry,
      ]),
    });

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("sftp_upload_file", {
        request: expect.objectContaining({ remotePath: "/srv/kept.txt" }),
      }),
    );
    expect(
      mockInvoke.mock.calls.filter(([command]) => command === "sftp_upload_file"),
    ).toHaveLength(1);
    expect(
      useSessionStore.getState().message?.text,
    ).toBe("Directory drag-and-drop is not supported yet");
  });

  it("asks for overwrite confirmation and skips when rejected", async () => {
    remoteEntries = [remoteFile("exists.dat")];
    mockConfirm.mockResolvedValue(false);
    mockInvoke.mockImplementation((command, args) => {
      if (command === "load_app_settings") {
        return Promise.resolve({ recentLocalPath: "C:\\Users\\me" });
      }
      if (command === "list_local_dir") {
        return Promise.resolve({ path: (args as { path: string }).path, entries: [] });
      }
      if (command === "sftp_list_dir") {
        return Promise.resolve({ path: "/srv", entries: remoteEntries });
      }
      if (command === "local_path_exists") {
        return Promise.resolve(true);
      }
      return Promise.resolve(undefined);
    });

    render(<SftpBrowser />);

    await waitFor(() => expect(screen.getByText("/srv")).toBeInTheDocument());
    fireEvent.drop(screen.getByTestId("local-drop-zone"), {
      dataTransfer: dragData(REMOTE_DRAG_TYPE, [remoteEntries[0]]),
    });

    await waitFor(() => expect(mockConfirm).toHaveBeenCalled());
    expect(
      mockInvoke.mock.calls.some(([command]) => command === "sftp_download_file"),
    ).toBe(false);
    expect(screen.getByText("canceled")).toBeInTheDocument();
  });

  it("marks the task as failed when transfer command fails", async () => {
    const file = localFile("broken.txt");
    mockInvoke.mockImplementation((command, args) => {
      if (command === "load_app_settings") {
        return Promise.resolve({ recentLocalPath: "C:\\Users\\me" });
      }
      if (command === "list_local_dir") {
        return Promise.resolve({ path: (args as { path: string }).path, entries: [] });
      }
      if (command === "sftp_list_dir") {
        return Promise.resolve({ path: "/srv", entries: [] });
      }
      if (command === "sftp_path_exists") {
        return Promise.resolve(false);
      }
      if (command === "sftp_upload_file") {
        return Promise.reject("remote disk full");
      }
      return Promise.resolve(undefined);
    });

    render(<SftpBrowser />);

    await waitFor(() => expect(screen.getByText("/srv")).toBeInTheDocument());
    fireEvent.drop(screen.getByTestId("remote-drop-zone"), {
      dataTransfer: dragData(LOCAL_DRAG_TYPE, [file]),
    });

    await waitFor(() => expect(screen.getByText("failed")).toBeInTheDocument());
    expect(screen.getByText("remote disk full")).toBeInTheDocument();
  });
});
