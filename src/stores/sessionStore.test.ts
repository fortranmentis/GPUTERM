import { beforeEach, describe, expect, it } from "vitest";
import {
  selectActiveTelemetry,
  selectIsActiveConnected,
  useSessionStore,
} from "./sessionStore";
import type { RemoteTelemetry } from "../types/gpu";

const sampleTelemetry = (hostname: string): RemoteTelemetry =>
  ({ hostname, gpus: [], disks: [] }) as unknown as RemoteTelemetry;

describe("useSessionStore", () => {
  beforeEach(() => {
    useSessionStore.setState({
      sessions: [],
      activeSessionId: null,
      terminalViewRevision: 0,
      connectedSessionIds: [],
      terminalIdsBySession: {},
      telemetryBySession: {},
      lostSessionIds: [],
      message: null,
      sftpRequest: null,
    });
  });

  it("connects and disconnects sessions cleanly", () => {
    const store = useSessionStore.getState();
    store.addConnectedSession("s1");
    store.addConnectedSession("s2", "term-2");
    store.setSessionTelemetry("s1", sampleTelemetry("host-1"));
    store.setSessionTelemetry("s2", sampleTelemetry("host-2"));

    // Adding an already connected session does not duplicate connectedSessionIds.
    store.addConnectedSession("s1", "term-1b");
    expect(useSessionStore.getState().connectedSessionIds).toEqual(["s1", "s2"]);
    expect(useSessionStore.getState().terminalIdsBySession).toEqual({
      s1: ["term-1b"],
      s2: ["term-2"],
    });

    store.removeConnectedSession("s1");
    const after = useSessionStore.getState();
    expect(after.connectedSessionIds).toEqual(["s2"]);
    expect(after.terminalIdsBySession).toEqual({ s2: ["term-2"] });
    expect(after.telemetryBySession).toEqual({ s2: sampleTelemetry("host-2") });
  });

  it("adds and removes terminal panes and disconnects when the last pane closes", () => {
    const store = useSessionStore.getState();
    store.addConnectedSession("s1", "t1");
    store.setSessionTelemetry("s1", sampleTelemetry("host-1"));

    store.addTerminalPane("s1", "t2");
    store.addTerminalPane("s1", "t2"); // duplicate ignored
    expect(useSessionStore.getState().terminalIdsBySession.s1).toEqual(["t1", "t2"]);

    // Removing one pane keeps the session connected with remaining panes.
    store.removeTerminalPane("s1", "t1");
    expect(useSessionStore.getState().terminalIdsBySession.s1).toEqual(["t2"]);
    expect(useSessionStore.getState().connectedSessionIds).toEqual(["s1"]);
    expect(useSessionStore.getState().telemetryBySession.s1).toBeDefined();

    // Removing the last pane disconnects the session and clears its telemetry.
    store.removeTerminalPane("s1", "t2");
    const after = useSessionStore.getState();
    expect(after.terminalIdsBySession.s1).toBeUndefined();
    expect(after.connectedSessionIds).toEqual([]);
    expect(after.telemetryBySession.s1).toBeUndefined();
  });

  it("switches active sessions and updates selectors and view revision", () => {
    const store = useSessionStore.getState();
    store.addConnectedSession("s1");
    store.setSessionTelemetry("s1", sampleTelemetry("gpu-1"));

    expect(selectIsActiveConnected(useSessionStore.getState())).toBe(false);
    expect(selectActiveTelemetry(useSessionStore.getState())).toBeNull();

    store.setActiveSession("s1");
    expect(selectIsActiveConnected(useSessionStore.getState())).toBe(true);
    expect(selectActiveTelemetry(useSessionStore.getState())).toEqual(
      sampleTelemetry("gpu-1"),
    );
    expect(useSessionStore.getState().terminalViewRevision).toBe(0);

    store.showSession("s2");
    expect(useSessionStore.getState().activeSessionId).toBe("s2");
    expect(useSessionStore.getState().terminalViewRevision).toBe(1);
    expect(selectIsActiveConnected(useSessionStore.getState())).toBe(false);
    expect(selectActiveTelemetry(useSessionStore.getState())).toBeNull();
  });

  it("tracks lost sessions and SFTP path requests", () => {
    const store = useSessionStore.getState();
    store.markSessionLost("s1");
    store.markSessionLost("s1");
    expect(useSessionStore.getState().lostSessionIds).toEqual(["s1"]);

    store.clearSessionLost("s1");
    expect(useSessionStore.getState().lostSessionIds).toEqual([]);

    store.requestSftpPath("s1", "/data/models");
    expect(useSessionStore.getState().sftpRequest).toEqual({
      sessionId: "s1",
      path: "/data/models",
    });
    store.clearSftpRequest();
    expect(useSessionStore.getState().sftpRequest).toBeNull();
  });
});
