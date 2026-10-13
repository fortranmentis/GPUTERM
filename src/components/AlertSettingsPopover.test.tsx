import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useRef } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { AlertSettingsPopover, alertSettingsProblem } from "./AlertSettingsPopover";
import { useAlertStore } from "../stores/alertStore";
import type { AlertEvent, AlertSettings } from "../types/alerts";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-notification", () => ({
  isPermissionGranted: vi.fn(() => Promise.resolve(true)),
  requestPermission: vi.fn(() => Promise.resolve("granted")),
  sendNotification: vi.fn(),
}));

const mockInvoke = vi.mocked(invoke);

const rule = (threshold: number, hysteresis = 5) => ({
  enabled: true,
  threshold,
  hysteresis,
  sustainSecs: 30,
});

const settings: AlertSettings = {
  enabled: true,
  osNotifications: true,
  notifyWhenFocused: false,
  cooldownSecs: 600,
  gpuTemp: rule(85),
  gpuVram: rule(95),
  gpuIdle: { enabled: false, utilBelow: 5, minVramPercent: 10, sustainSecs: 900 },
  memory: rule(90),
  disk: rule(90, 2),
  llmCritical: true,
};

function Harness() {
  const anchorRef = useRef<HTMLButtonElement | null>(null);
  return (
    <>
      <button ref={anchorRef} type="button">
        anchor
      </button>
      <AlertSettingsPopover anchorRef={anchorRef} onClose={() => undefined} />
    </>
  );
}

describe("AlertSettingsPopover", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    useAlertStore.setState({ settings, recent: [], active: {} });
  });

  it("saves edited thresholds and keeps the clear level in place", async () => {
    mockInvoke.mockImplementation(async (_command, args) => (args as { settings: AlertSettings }).settings);
    render(<Harness />);

    const level = screen.getByLabelText("GPU temperature alert level");
    expect(screen.getByLabelText("GPU temperature clears below")).toHaveValue(80);
    fireEvent.change(level, { target: { value: "80" } });
    expect(screen.getByLabelText("GPU temperature clears below")).toHaveValue(80);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("update_alert_settings", {
        settings: expect.objectContaining({
          gpuTemp: expect.objectContaining({ threshold: 80, hysteresis: 0 }),
        }),
      }),
    );
    await waitFor(() => expect(useAlertStore.getState().settings?.gpuTemp.threshold).toBe(80));
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  });

  it("shows a failed save and keeps the edit", async () => {
    mockInvoke.mockRejectedValue("disk full");
    render(<Harness />);
    fireEvent.click(screen.getByRole("checkbox", { name: "Memory" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("disk full");
    expect(screen.getByRole("checkbox", { name: "Memory" })).not.toBeChecked();
    expect(useAlertStore.getState().settings?.memory.enabled).toBe(true);
  });

  it("refuses a clear level above the alert level", () => {
    render(<Harness />);
    fireEvent.change(screen.getByLabelText("Memory clears below"), { target: { value: "95" } });
    expect(screen.getByRole("alert")).toHaveTextContent("must not be above");
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(alertSettingsProblem(settings)).toBeNull();
  });

  it("lists recent alerts", () => {
    const fired: AlertEvent = {
      id: "1",
      scope: "s",
      hostLabel: "lab-a100",
      kind: "gpu-temp",
      subject: "GPU-a",
      state: "fired",
      value: 88,
      threshold: 85,
      at: 1_900_000_000,
      title: "GPU 0 (A100) is hot",
      body: "88°C (alert at 85°C)",
      notify: true,
      osDelivered: true,
    };
    useAlertStore.getState().push(fired);
    render(<Harness />);
    expect(screen.getByText(/GPU 0 \(A100\) is hot/)).toBeInTheDocument();
    expect(useAlertStore.getState().active.s).toHaveProperty("gpu-temp:GPU-a");
    useAlertStore.getState().push({ ...fired, id: "2", state: "resolved" });
    expect(useAlertStore.getState().active.s).toEqual({});
  });
});
