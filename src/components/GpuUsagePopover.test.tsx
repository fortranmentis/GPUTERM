import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useRef, useState } from "react";
import { describe, expect, it, vi } from "vitest";
import type { GpuDetailMetric } from "../types/resourceDetails";
import { GpuUsagePopover } from "./GpuUsagePopover";

const gpu0: GpuDetailMetric = {
  index: 0,
  name: "GPU Zero",
  uuid: "GPU-uuid-zero",
  driverVersion: "550.1",
  gpuUtilPercent: 10,
  memoryUtilPercent: 25,
  memoryTotalMiB: 16384,
  memoryUsedMiB: 4096,
  memoryFreeMiB: 12288,
  temperatureC: 50,
  powerDrawW: 100,
  powerLimitW: 200,
  fanSpeedPercent: 20,
  graphicsClockMHz: 1200,
  memoryClockMHz: 1000,
  pciBusId: "0000:01:00.0",
  persistenceMode: "Enabled",
  migMode: "Disabled",
  processes: [
    {
      gpuIndex: 0,
      gpuUuid: "GPU-uuid-zero",
      pid: 100,
      user: "alice",
      processName: "gpu-zero-worker",
      command: "python zero.py",
      usedMemoryMiB: 2048,
    },
    {
      gpuIndex: 1,
      gpuUuid: "GPU-uuid-one",
      pid: 999,
      user: "wrong",
      processName: "wrong-gpu-process",
      command: "python wrong.py",
      usedMemoryMiB: 1024,
    },
  ],
};

const gpu1: GpuDetailMetric = {
  ...gpu0,
  index: 1,
  name: "GPU One",
  uuid: "GPU-uuid-one",
  gpuUtilPercent: 80,
  memoryUtilPercent: 75,
  memoryTotalMiB: 24576,
  memoryUsedMiB: 18432,
  memoryFreeMiB: 6144,
  temperatureC: 72,
  powerDrawW: 260,
  powerLimitW: 300,
  pciBusId: "0000:02:00.0",
  processes: [
    {
      gpuIndex: 1,
      gpuUuid: "GPU-uuid-one",
      pid: 200,
      user: "bob",
      processName: "gpu-one-trainer",
      command: "python train.py",
      usedMemoryMiB: 16384,
    },
  ],
};

function Harness({
  metrics,
  initialGpuUuid = null,
}: {
  metrics: GpuDetailMetric[];
  initialGpuUuid?: string | null;
}) {
  const anchorRef = useRef<HTMLButtonElement | null>(null);
  const [selectedGpuUuid, setSelectedGpuUuid] = useState(initialGpuUuid);

  return (
    <>
      <button ref={anchorRef} type="button">GPU details anchor</button>
      <output data-testid="selected-gpu-uuid">{selectedGpuUuid ?? "none"}</output>
      <GpuUsagePopover
        metrics={metrics}
        selectedGpuUuid={selectedGpuUuid}
        onSelectedGpuUuidChange={setSelectedGpuUuid}
        loading={false}
        anchorRef={anchorRef}
        onClose={vi.fn()}
      />
    </>
  );
}

describe("GpuUsagePopover", () => {
  it("renders summary, gauges, and processes for the selected GPU", async () => {
    render(<Harness metrics={[gpu0, gpu1]} initialGpuUuid={gpu0.uuid} />);

    expect(screen.getByText("GPU Zero")).toBeInTheDocument();
    expect(screen.getByText("4.00 GiB / 16.0 GiB")).toBeInTheDocument();
    expect(screen.getByText("100 W / 200 W")).toBeInTheDocument();
    expect(screen.getByText("50 C")).toBeInTheDocument();
    expect(screen.getByText("gpu-zero-worker")).toBeInTheDocument();
    expect(screen.queryByText("wrong-gpu-process")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("tab", { name: "GPU1" }));

    await waitFor(() => expect(screen.getByTestId("selected-gpu-uuid")).toHaveTextContent(gpu1.uuid));
    expect(screen.getByText("GPU One")).toBeInTheDocument();
    expect(screen.getByText("18.0 GiB / 24.0 GiB")).toBeInTheDocument();
    expect(screen.getByText("260 W / 300 W")).toBeInTheDocument();
    expect(screen.getByText("72 C")).toBeInTheDocument();
    expect(screen.getByText("gpu-one-trainer")).toBeInTheDocument();
    expect(screen.queryByText("gpu-zero-worker")).not.toBeInTheDocument();
  });

  it("keeps the selected GPU by UUID when telemetry is reordered", () => {
    const { rerender } = render(
      <Harness metrics={[gpu0, gpu1]} initialGpuUuid={gpu1.uuid} />,
    );
    const reorderedGpu1 = { ...gpu1, index: 7, gpuUtilPercent: 81 };

    rerender(<Harness metrics={[reorderedGpu1, gpu0]} initialGpuUuid={gpu1.uuid} />);

    expect(screen.getByText("GPU One")).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "GPU7" })).toHaveAttribute("aria-selected", "true");
  });

  it("falls back to the first GPU when the selected UUID disappears", async () => {
    const { rerender } = render(
      <Harness metrics={[gpu0, gpu1]} initialGpuUuid={gpu1.uuid} />,
    );

    rerender(<Harness metrics={[gpu0]} initialGpuUuid={gpu1.uuid} />);

    await waitFor(() => expect(screen.getByTestId("selected-gpu-uuid")).toHaveTextContent(gpu0.uuid));
    expect(screen.getByText("GPU Zero")).toBeInTheDocument();
  });

  it("hides the selector when only one GPU is present", () => {
    render(<Harness metrics={[gpu0]} initialGpuUuid={gpu0.uuid} />);

    expect(screen.queryByRole("tablist", { name: "GPU selector" })).not.toBeInTheDocument();
    expect(screen.getByText("GPU Zero")).toBeInTheDocument();
  });

  it("shows a no-GPU message instead of selector and details", () => {
    render(<Harness metrics={[]} />);

    expect(screen.getByText("No NVIDIA GPU detected")).toBeInTheDocument();
    expect(screen.queryByRole("tablist", { name: "GPU selector" })).not.toBeInTheDocument();
    expect(screen.queryByText("Driver")).not.toBeInTheDocument();
  });

  it("renders processes without GPU identity or with duplicate pids without key collisions", () => {
    const anonymousProcess = {
      gpuIndex: null,
      gpuUuid: null,
      pid: 4242,
      user: "alice",
      processName: "shared-worker",
      command: "python shared.py",
      usedMemoryMiB: 512,
    };
    const gpuWithAnonymousProcesses = {
      ...gpu0,
      processes: [anonymousProcess, { ...anonymousProcess, usedMemoryMiB: 256 }],
    };
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
    try {
      render(<Harness metrics={[gpuWithAnonymousProcesses]} initialGpuUuid={gpu0.uuid} />);

      expect(screen.getAllByText("shared-worker")).toHaveLength(2);
      const keyWarnings = consoleError.mock.calls.filter((call) =>
        String(call[0]).includes("same key"),
      );
      expect(keyWarnings).toHaveLength(0);
    } finally {
      consoleError.mockRestore();
    }
  });
});
