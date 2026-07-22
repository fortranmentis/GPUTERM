import { beforeEach, describe, expect, it, vi } from "vitest";

const coreMock = vi.hoisted(() => {
  return {
    invoke: vi.fn(),
  };
});

vi.mock("@tauri-apps/api/core", () => coreMock);

import { startNativeFileDrag } from "./nativeFileDrag";

describe("startNativeFileDrag", () => {
  beforeEach(() => {
    coreMock.invoke.mockReset();
    coreMock.invoke.mockResolvedValue(undefined);
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        arrayBuffer: () => Promise.resolve(Uint8Array.from([137, 80, 78, 71]).buffer),
      }),
    );
  });

  it("passes absolute file paths to the native drag command", async () => {
    await startNativeFileDrag(["/tmp/gputerm/drag-out/report.txt"]);

    expect(coreMock.invoke).toHaveBeenCalledWith(
      "start_native_file_drag",
      expect.objectContaining({
        paths: ["/tmp/gputerm/drag-out/report.txt"],
        image: expect.stringMatching(/^data:image\/png;base64,/),
      }),
    );
  });

  it("rejects an empty drag without invoking the plugin", async () => {
    await expect(startNativeFileDrag([])).rejects.toThrow(
      "No prepared local files are available to drag",
    );
    expect(coreMock.invoke).not.toHaveBeenCalled();
  });
});
