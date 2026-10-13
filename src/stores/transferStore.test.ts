import { beforeEach, describe, expect, it } from "vitest";
import { progressToMessage, useTransferStore } from "./transferStore";
import type { SftpProgressPayload } from "../types/session";

function payload(patch: Partial<SftpProgressPayload>): SftpProgressPayload {
  return {
    transferId: "t1",
    sessionId: "s1",
    operation: "upload",
    remotePath: "/srv/a.txt",
    localPath: "/tmp/a.txt",
    transferredBytes: 5,
    totalBytes: 10,
    done: true,
    canceled: false,
    error: null,
    ...patch,
  };
}

describe("progressToMessage", () => {
  it("shows nothing until the transfer is done", () => {
    expect(progressToMessage(payload({ done: false }))).toBeNull();
  });

  it("reports a cancel as info, not as an error", () => {
    expect(
      progressToMessage(payload({ canceled: true, error: "Transfer canceled" })),
    ).toEqual({ kind: "info", text: "upload canceled: /srv/a.txt" });
  });

  it("reports a failure as an error", () => {
    expect(progressToMessage(payload({ error: "disk full" }))).toEqual({
      kind: "error",
      text: "disk full",
    });
  });

  it("reports completion as success", () => {
    expect(progressToMessage(payload({}))).toEqual({
      kind: "success",
      text: "upload complete: /srv/a.txt",
    });
  });
});

describe("updateFromProgress", () => {
  beforeEach(() => {
    useTransferStore.setState({
      tasks: [
        {
          id: "t1",
          sessionId: "s1",
          direction: "upload",
          filename: "a.txt",
          sourcePath: "/tmp/a.txt",
          targetPath: "/srv/a.txt",
          totalBytes: 10,
          transferredBytes: 0,
          progressPercent: 0,
          status: "running",
        },
      ],
      activeDrag: null,
    });
  });

  it("marks a canceled transfer canceled", () => {
    useTransferStore
      .getState()
      .updateFromProgress(payload({ canceled: true, error: "Transfer canceled" }));
    expect(useTransferStore.getState().tasks[0].status).toBe("canceled");
  });
});
