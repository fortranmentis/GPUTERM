import { describe, expect, it } from "vitest";
import {
  appendPendingOutput,
  clearPendingOutput,
  takePendingOutput,
} from "./terminalBuffer";

describe("terminal pending-output buffer", () => {
  it("accumulates and takes output per session", () => {
    const buffers = new Map<string, string>();
    appendPendingOutput(buffers, "a", "Welcome ");
    appendPendingOutput(buffers, "a", "to lab\r\n");
    appendPendingOutput(buffers, "b", "other");

    expect(takePendingOutput(buffers, "a")).toBe("Welcome to lab\r\n");
    expect(takePendingOutput(buffers, "a")).toBeNull();
    expect(takePendingOutput(buffers, "b")).toBe("other");
  });

  it("keeps only the tail of oversized entries", () => {
    const buffers = new Map<string, string>();
    appendPendingOutput(buffers, "a", "x".repeat(300 * 1024));
    appendPendingOutput(buffers, "a", "END");

    const pending = takePendingOutput(buffers, "a");
    expect(pending?.length).toBe(256 * 1024);
    expect(pending?.endsWith("END")).toBe(true);
  });

  it("evicts the least recently written session beyond the entry cap", () => {
    const buffers = new Map<string, string>();
    for (let index = 0; index < 9; index += 1) {
      appendPendingOutput(buffers, `session-${index}`, "data");
    }

    expect(buffers.size).toBe(8);
    expect(takePendingOutput(buffers, "session-0")).toBeNull();
    expect(takePendingOutput(buffers, "session-8")).toBe("data");
  });

  it("refreshes recency on append so active sessions survive eviction", () => {
    const buffers = new Map<string, string>();
    appendPendingOutput(buffers, "keep", "1");
    for (let index = 0; index < 7; index += 1) {
      appendPendingOutput(buffers, `fill-${index}`, "data");
    }
    appendPendingOutput(buffers, "keep", "2");
    appendPendingOutput(buffers, "new", "data");

    expect(takePendingOutput(buffers, "keep")).toBe("12");
  });

  it("clears a session buffer", () => {
    const buffers = new Map<string, string>();
    appendPendingOutput(buffers, "a", "logout\r\n");
    clearPendingOutput(buffers, "a");

    expect(takePendingOutput(buffers, "a")).toBeNull();
  });
});
