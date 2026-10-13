import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useRef, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { commandMismatch, KillProcessDialog } from "./KillProcessDialog";
import { ResourceDetailPopover } from "./ResourceDetailPopover";
import { CpuDetailContent } from "./CpuUsagePopover";
import type { ProcessIdentity, SignalOutcome } from "../types/processControl";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const mockInvoke = vi.mocked(invoke);

const target = { pid: 4242, user: "alice", command: "python" };
const live: ProcessIdentity = {
  pid: 4242,
  alive: true,
  user: "alice",
  command: "python train.py",
  startMarker: "987654",
};

function script(steps: { inspect?: ProcessIdentity[]; signal?: SignalOutcome[] }) {
  const inspections = [...(steps.inspect ?? [])];
  const signals = [...(steps.signal ?? [])];
  mockInvoke.mockImplementation(async (command) => {
    if (command === "inspect_process") return inspections.length > 1 ? inspections.shift() : inspections[0];
    if (command === "signal_process") return signals.shift();
    return null;
  });
}

function renderDialog(onDone = vi.fn()) {
  render(
    <KillProcessDialog sessionId="s" target={target} onClose={vi.fn()} onDone={onDone} />,
  );
  return onDone;
}

describe("KillProcessDialog", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("ends the process with its start marker and reports when it is gone", async () => {
    script({
      inspect: [live, live, { ...live, alive: false, startMarker: null }],
      signal: [{ status: "sent", message: null }],
    });
    const onDone = renderDialog();
    expect(await screen.findByText("python train.py")).toBeInTheDocument();
    vi.useFakeTimers({ shouldAdvanceTime: true });

    fireEvent.click(screen.getByRole("button", { name: "End process" }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("signal_process", {
        sessionId: "s",
        pid: 4242,
        signal: "term",
        expectedStartMarker: "987654",
      }),
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2_500);
    });
    expect(await screen.findByText("Process 4242 has ended.")).toBeInTheDocument();
    expect(onDone).toHaveBeenCalled();
  });

  it("offers a forced stop when the process keeps running", async () => {
    script({
      inspect: [live],
      signal: [
        { status: "sent", message: null },
        { status: "sent", message: null },
      ],
    });
    renderDialog();
    await screen.findByText("python train.py");
    vi.useFakeTimers({ shouldAdvanceTime: true });
    fireEvent.click(screen.getByRole("button", { name: "End process" }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(11_000);
    });
    const force = await screen.findByRole("button", { name: "Force stop" });
    fireEvent.click(force);
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith(
        "signal_process",
        expect.objectContaining({ signal: "kill" }),
      ),
    );
  });

  it("explains a process owned by someone else", async () => {
    script({
      inspect: [live],
      signal: [{ status: "permission_denied", message: "This process belongs to another user." }],
    });
    renderDialog();
    await screen.findByText("python train.py");
    fireEvent.click(screen.getByRole("button", { name: "End process" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("belongs to another user");
  });

  it("refuses when the PID now belongs to a different process", async () => {
    script({ inspect: [live], signal: [{ status: "identity_changed", message: null }] });
    const onDone = renderDialog();
    await screen.findByText("python train.py");
    fireEvent.click(screen.getByRole("button", { name: "End process" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("different process");
    expect(screen.queryByRole("button", { name: "End process" })).toBeNull();
    expect(onDone).not.toHaveBeenCalled();
  });

  it("does not offer to end a process that is not visible", async () => {
    script({ inspect: [{ ...live, alive: false, startMarker: null, command: null }] });
    renderDialog();
    expect(await screen.findByText(/no longer exists/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "End process" })).toBeNull();
  });

  it("warns when the PID runs a different program than the row showed", () => {
    expect(commandMismatch("python", "/usr/bin/python3 train.py")).toBe(false);
    expect(commandMismatch("python", "nginx: worker")).toBe(true);
    expect(commandMismatch(null, "anything")).toBe(false);
  });
});

describe("a dialog opened from a popover", () => {
  function Harness() {
    const anchorRef = useRef<HTMLButtonElement | null>(null);
    const [open, setOpen] = useState(true);
    return (
      <>
        <button ref={anchorRef} type="button">
          anchor
        </button>
        {open && (
          <ResourceDetailPopover
            anchorRef={anchorRef}
            ariaLabel="CPU details"
            title="CPU details"
            icon={null}
            onClose={() => setOpen(false)}
          >
            <span>table</span>
          </ResourceDetailPopover>
        )}
        <div data-keep-popover>
          <button type="button">dialog button</button>
        </div>
      </>
    );
  }

  it("keeps the popover open while the dialog is clicked or Escape closes it", () => {
    render(<Harness />);
    fireEvent.mouseDown(screen.getByRole("button", { name: "dialog button" }));
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.getByText("table")).toBeInTheDocument();
  });
});

describe("process table end buttons", () => {
  const metric = {
    modelName: "EPYC",
    usagePercent: 10,
    loadAvg1: 1,
    loadAvg5: 1,
    loadAvg15: 1,
    totalCores: 8,
    onlineCores: 8,
    avgClockGhz: 3,
    uptimeSeconds: 100,
    logicalCoreUsagePercent: [],
    topProcesses: [{ pid: 4242, user: "alice", command: "python", cpuPercent: 99 }],
  };

  it("appear only when the view can end processes", () => {
    const { unmount } = render(<CpuDetailContent metric={metric} thermal={null} />);
    expect(screen.queryByRole("button", { name: "End process 4242" })).toBeNull();
    unmount();

    const onKill = vi.fn();
    render(<CpuDetailContent metric={metric} thermal={null} onKillProcess={onKill} />);
    fireEvent.click(screen.getByRole("button", { name: "End process 4242" }));
    expect(onKill).toHaveBeenCalledWith({ pid: 4242, user: "alice", command: "python" });
  });
});
