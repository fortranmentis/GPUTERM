import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { confirm, open } from "@tauri-apps/plugin-dialog";
import { readClipboardLocalPaths, SftpBrowser } from "./SftpBrowser";
import { useSessionStore } from "../stores/sessionStore";
import { useTransferStore } from "../stores/transferStore";
import type { LocalEntry, SftpEntry } from "../types/session";

const nativeDropMock = vi.hoisted(() => ({
  handler: null as
    | ((event: {
        payload:
          | { type: "enter" | "drop"; paths: string[]; position: { x: number; y: number } }
          | { type: "over"; position: { x: number; y: number } }
          | { type: "leave" };
      }) => void)
    | null,
}));
const nativeFileDragMock = vi.hoisted(() => ({
  start: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));

vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: vi.fn((handler) => {
      nativeDropMock.handler = handler;
      return Promise.resolve(() => undefined);
    }),
  }),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  confirm: vi.fn(),
  open: vi.fn(),
}));

vi.mock("../utils/nativeFileDrag", () => ({
  startNativeFileDrag: nativeFileDragMock.start,
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

function localDirectory(name: string): LocalEntry {
  return {
    name,
    path: `C:\\Users\\me\\${name}`,
    entryType: "directory",
    size: null,
    modifiedTime: null,
  };
}

function remoteDirectory(name: string): SftpEntry {
  return {
    name,
    path: `/srv/${name}`,
    type: "directory",
    size: null,
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
    nativeFileDragMock.start.mockReset();
    nativeFileDragMock.start.mockResolvedValue(undefined);
    nativeDropMock.handler = null;
    localEntries = [];
    remoteEntries = [];
    useTransferStore.setState({ tasks: [], activeDrag: null });
    useSessionStore.setState({
      activeSessionId: "session-1",
      connectedSessionIds: ["session-1"],
      terminalIdsBySession: { "session-1": ["terminal-1"] },
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
      if (command === "describe_local_paths") {
        return Promise.resolve(localEntries);
      }
      if (command === "sftp_create_drag_out_paths") {
        const remotePaths = (args as { remotePaths: string[] }).remotePaths;
        return Promise.resolve(
          remotePaths.map((remotePath) => ({
            remotePath,
            localPath: `/tmp/gputerm/drag-out/${remotePath.split("/").at(-1)}`,
          })),
        );
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

  it("shows the folder-name editor only after the icon-only mkdir button", async () => {
    render(<SftpBrowser />);

    await screen.findByText("/srv");
    expect(screen.getByRole("button", { name: "Open remote path" })).toBeVisible();
    expect(
      screen.queryByRole("textbox", { name: "New remote folder name" }),
    ).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Create remote folder" }));
    const input = screen.getByRole("textbox", { name: "New remote folder name" });
    fireEvent.change(input, { target: { value: "reports" } });
    fireEvent.click(screen.getByRole("button", { name: "Confirm new folder" }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("sftp_mkdir", {
        request: {
          sessionId: "session-1",
          remotePath: "/srv/reports",
        },
      }),
    );
    await waitFor(() => expect(input).not.toBeInTheDocument());
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

  it("uploads native desktop drops with absolute Debian, Windows, and macOS paths", async () => {
    const paths = [
      "/home/me/debian.png",
      "C:\\Users\\me\\windows.png",
      "/Users/me/macos.png",
    ];
    localEntries = paths.map((entryPath) => ({
      name: entryPath.split(/[\\/]/).at(-1) ?? entryPath,
      path: entryPath,
      entryType: "file",
      size: 10,
      modifiedTime: null,
    }));
    render(<SftpBrowser />);

    await waitFor(() => expect(nativeDropMock.handler).not.toBeNull());
    const remoteDropZone = screen.getByTestId("remote-drop-zone");
    vi.spyOn(remoteDropZone, "getBoundingClientRect").mockReturnValue(
      domRect(0, 0, 500, 500),
    );

    await act(async () => {
      nativeDropMock.handler?.({
        payload: {
          type: "drop",
          paths,
          position: { x: 100, y: 100 },
        },
      });
    });

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("describe_local_paths", { paths }),
    );
    await waitFor(() =>
      expect(
        mockInvoke.mock.calls.filter(([command]) => command === "sftp_upload_file"),
      ).toHaveLength(3),
    );
    expect(
      mockInvoke.mock.calls
        .filter(([command]) => command === "sftp_upload_file")
        .map(([, args]) =>
          (args as { request: { localPath: string } }).request.localPath,
        ),
    ).toEqual(paths);
  });

  it("never submits a browser file name as though it were an absolute path", async () => {
    render(<SftpBrowser />);

    await screen.findByText("/srv");
    fireEvent.drop(screen.getByTestId("remote-drop-zone"), {
      dataTransfer: {
        files: [new File(["image"], "wind.png", { type: "image/png" })],
        getData: () => "",
      },
    });

    await waitFor(() =>
      expect(useSessionStore.getState().message?.text).toBe(
        "The dropped item path was unavailable. Please retry the desktop file drop.",
      ),
    );
    expect(
      mockInvoke.mock.calls.some(([command]) => command === "sftp_upload_file"),
    ).toBe(false);
  });

  it("keeps local-to-remote drag working with pointer events", async () => {
    localEntries = [localFile("pointer-upload.txt")];
    render(<SftpBrowser />);

    const remoteDropZone = await screen.findByTestId("remote-drop-zone");
    vi.spyOn(remoteDropZone, "getBoundingClientRect").mockReturnValue(
      domRect(0, 0, 500, 500),
    );
    const localFileButton = await screen.findByRole("button", {
      name: /pointer-upload\.txt/i,
    });

    fireEvent(localFileButton, pointerEvent("pointerdown", {
      button: 0,
      pointerId: 7,
      clientX: 700,
      clientY: 700,
    }));
    fireEvent(document, pointerEvent("pointermove", {
      pointerId: 7,
      clientX: 100,
      clientY: 100,
    }));
    fireEvent(document, pointerEvent("pointerup", {
      pointerId: 7,
      clientX: 100,
      clientY: 100,
    }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("sftp_upload_file", {
        request: expect.objectContaining({
          localPath: "C:\\Users\\me\\pointer-upload.txt",
          remotePath: "/srv/pointer-upload.txt",
        }),
      }),
    );
  });

  it("uploads files pasted from Nautilus on the remote panel", async () => {
    localEntries = [localFile("copied file.txt")];
    render(<SftpBrowser />);

    await waitFor(() => expect(screen.getByText("/srv")).toBeInTheDocument());
    fireEvent.paste(screen.getByTestId("remote-drop-zone"), {
      clipboardData: {
        files: [],
        getData: (type: string) =>
          type === "x-special/gnome-copied-files"
            ? "copy\nfile:///home/me/copied%20file.txt"
            : "",
      },
    });

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("describe_local_paths", {
        paths: ["/home/me/copied file.txt"],
      }),
    );
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("sftp_upload_file", {
        request: expect.objectContaining({
          remotePath: "/srv/copied file.txt",
        }),
      }),
    );
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

  it("keeps remote-to-local drag working with pointer events", async () => {
    remoteEntries = [remoteFile("pointer-download.log")];
    render(<SftpBrowser />);

    const localDropZone = await screen.findByTestId("local-drop-zone");
    vi.spyOn(localDropZone, "getBoundingClientRect").mockReturnValue(
      domRect(500, 0, 500, 500),
    );
    const remoteFileButton = await screen.findByRole("button", {
      name: /pointer-download\.log/i,
    });

    fireEvent(remoteFileButton, pointerEvent("pointerdown", {
      button: 0,
      pointerId: 8,
      clientX: 100,
      clientY: 700,
    }));
    fireEvent(document, pointerEvent("pointermove", {
      pointerId: 8,
      clientX: 600,
      clientY: 100,
    }));
    fireEvent(document, pointerEvent("pointerup", {
      pointerId: 8,
      clientX: 600,
      clientY: 100,
    }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("sftp_download_file", {
        request: expect.objectContaining({
          remotePath: "/srv/pointer-download.log",
          localPath: "C:\\Users\\me\\pointer-download.log",
        }),
      }),
    );
  });

  it("materializes remote files and starts a native drag at the window edge", async () => {
    remoteEntries = [remoteFile("desktop-export.log")];
    render(<SftpBrowser />);

    const remoteFileButton = await screen.findByRole("button", {
      name: /desktop-export\.log/i,
    });
    fireEvent(remoteFileButton, pointerEvent("pointerdown", {
      button: 0,
      pointerId: 9,
      clientX: 300,
      clientY: 300,
    }));
    fireEvent(document, pointerEvent("pointermove", {
      pointerId: 9,
      clientX: 2,
      clientY: 300,
    }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("sftp_create_drag_out_paths", {
        remotePaths: ["/srv/desktop-export.log"],
      }),
    );
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("sftp_download_file", {
        request: expect.objectContaining({
          remotePath: "/srv/desktop-export.log",
          localPath: "/tmp/gputerm/drag-out/desktop-export.log",
        }),
      }),
    );
    await waitFor(() =>
      expect(nativeFileDragMock.start).toHaveBeenCalledWith([
        "/tmp/gputerm/drag-out/desktop-export.log",
      ]),
    );
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

  it("uploads folders and files from a mixed drop", async () => {
    const directoryEntry = localDirectory("logs");
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
    ).toHaveLength(3);
    expect(
      mockInvoke.mock.calls
        .filter(([command]) => command === "sftp_upload_file")
        .map(([, args]) =>
          (args as { request: { remotePath: string } }).request.remotePath,
        ),
    ).toEqual(expect.arrayContaining(["/srv/logs", "/srv/cache", "/srv/kept.txt"]));
  });

  it("selects a local folder on one click and uploads it", async () => {
    localEntries = [localDirectory("models")];
    render(<SftpBrowser />);

    const folder = await screen.findByRole("button", { name: /models/i });
    fireEvent.click(folder);
    expect(folder).toHaveClass("selected");
    expect(mockInvoke).not.toHaveBeenCalledWith("list_local_dir", {
      path: "C:\\Users\\me\\models",
    });
    fireEvent.click(screen.getByRole("button", { name: /^upload$/i }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("sftp_upload_file", {
        request: expect.objectContaining({
          localPath: "C:\\Users\\me\\models",
          remotePath: "/srv/models",
        }),
      }),
    );
  });

  it("downloads a selected remote folder recursively", async () => {
    remoteEntries = [remoteDirectory("results")];
    render(<SftpBrowser />);

    const folder = await screen.findByRole("button", { name: /results/i });
    fireEvent.click(folder);
    expect(folder).toHaveClass("selected");
    fireEvent.click(screen.getByRole("button", { name: /^download$/i }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("sftp_download_file", {
        request: expect.objectContaining({
          remotePath: "/srv/results",
          localPath: "C:\\Users\\me\\results",
        }),
      }),
    );
  });

  it("resizes the remote/local storage ratio with the accessible separator", async () => {
    render(<SftpBrowser />);

    await screen.findByText("/srv");
    const separator = screen.getByRole("separator", {
      name: "Resize remote and local file panels",
    });
    expect(separator).toHaveAttribute("aria-valuenow", "58");

    fireEvent.keyDown(separator, { key: "ArrowDown" });
    expect(separator).toHaveAttribute("aria-valuenow", "63");
    fireEvent.keyDown(separator, { key: "ArrowUp" });
    expect(separator).toHaveAttribute("aria-valuenow", "58");
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

  it("exposes the SFTP panel close control", async () => {
    const onClose = vi.fn();

    render(<SftpBrowser onClose={onClose} />);
    await waitFor(() => expect(screen.getByText("/srv")).toBeInTheDocument());
    const closeButton = screen.getByRole("button", { name: "Close SFTP panel" });
    expect(closeButton).toHaveClass("ghost");
    expect(closeButton.querySelector(".lucide-panel-right-close")).toBeInTheDocument();
    fireEvent.click(closeButton);

    expect(onClose).toHaveBeenCalledOnce();
  });
});

describe("Nautilus clipboard parsing", () => {
  it("decodes GNOME copied-file URIs and ignores the copy marker", () => {
    const clipboard = {
      files: [],
      getData: (type: string) =>
        type === "x-special/gnome-copied-files"
          ? "copy\nfile:///home/me/alpha%20one.txt\nfile:///tmp/beta.log"
          : "",
    } as unknown as DataTransfer;

    expect(readClipboardLocalPaths(clipboard)).toEqual([
      "/home/me/alpha one.txt",
      "/tmp/beta.log",
    ]);
  });
});

function domRect(left: number, top: number, width: number, height: number): DOMRect {
  return {
    x: left,
    y: top,
    left,
    top,
    width,
    height,
    right: left + width,
    bottom: top + height,
    toJSON: () => ({}),
  };
}

function pointerEvent(
  type: "pointerdown" | "pointermove" | "pointerup",
  init: MouseEventInit & { pointerId: number },
) {
  const event = new MouseEvent(type, { bubbles: true, cancelable: true, ...init });
  Object.defineProperty(event, "pointerId", { value: init.pointerId });
  return event;
}
