import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { ThermalMetric, ThermalSensor } from "../types/gpu";
import { ThermalChip, ThermalSection } from "./ResourceDetailPopover";

function sensor(overrides: Partial<ThermalSensor> = {}): ThermalSensor {
  return {
    label: "Package id 0",
    source: "coretemp",
    temperatureC: 62,
    highC: null,
    criticalC: null,
    ...overrides,
  };
}

function metric(overrides: Partial<ThermalMetric> = {}): ThermalMetric {
  return { cpu: null, memory: null, disk: null, unsupported: [], ...overrides };
}

const WSL_REASON =
  "This host exposes no /sys/class/hwmon or /sys/class/thermal entries at all";

describe("ThermalSection", () => {
  it("draws a gauge per sensor when temperatures were read", () => {
    render(
      <ThermalSection
        kind="cpu"
        thermal={metric({
          cpu: {
            headlineC: 62,
            headlineLabel: "Package id 0",
            caveat: null,
            sensors: [sensor(), sensor({ label: "Core 0", temperatureC: 58 })],
          },
        })}
      />,
    );
    expect(screen.getByText("62 C")).toBeInTheDocument();
    expect(screen.getByText("58 C")).toBeInTheDocument();
    expect(screen.getByText("Core 0")).toBeInTheDocument();
    expect(screen.queryByText("Not available on this host")).not.toBeInTheDocument();
  });

  // The whole point of the three-state model: a host that cannot report a
  // temperature must not look like a host whose reading has not arrived yet.
  it("renders an unsupported host differently from one that has not reported yet", () => {
    const { container: unsupported } = render(
      <ThermalSection
        kind="cpu"
        thermal={metric({ unsupported: [{ category: "cpu", reason: WSL_REASON }] })}
      />,
    );
    expect(unsupported.querySelector(".thermal-unsupported")).not.toBeNull();
    expect(unsupported.querySelector(".thermal-gauge-grid")).toBeNull();
    expect(unsupported.textContent).toContain(WSL_REASON);

    const { container: pending } = render(<ThermalSection kind="cpu" thermal={null} />);
    expect(pending.querySelector(".thermal-unsupported")).toBeNull();
    expect(pending.querySelector(".thermal-gauge-grid")).not.toBeNull();
    expect(pending.textContent).toContain("n/a");
  });

  it("scopes unsupported to its own category", () => {
    // WSL2 reports all three, but a consumer desktop reports CPU and drives
    // while having no DIMM sensor at all.
    const thermal = metric({
      cpu: {
        headlineC: 62,
        headlineLabel: "Package id 0",
        caveat: null,
        sensors: [sensor()],
      },
      unsupported: [{ category: "memory", reason: "needs the jc42 module" }],
    });
    const { container } = render(<ThermalSection kind="cpu" thermal={thermal} />);
    expect(container.querySelector(".thermal-unsupported")).toBeNull();
    const { container: memory } = render(
      <ThermalSection kind="memory" thermal={thermal} />,
    );
    expect(memory.querySelector(".thermal-unsupported")).not.toBeNull();
  });

  it("never colours a reading that carries a caveat", () => {
    // Windows gives us an ACPI thermal zone, which is often a chassis sensor.
    // Showing it is useful; painting it red would lend it authority it lacks.
    const { container } = render(
      <ThermalSection
        kind="cpu"
        thermal={metric({
          cpu: {
            headlineC: 96,
            headlineLabel: "ACPI\\ThermalZone\\TZ00_0",
            caveat: "ACPI thermal zone, not the CPU die",
            sensors: [
              sensor({
                label: "ACPI\\ThermalZone\\TZ00_0",
                source: "acpi_thermal_zone",
                temperatureC: 96,
              }),
            ],
          },
        })}
      />,
    );
    expect(container.querySelector(".thermal-gauge-grid strong.critical")).toBeNull();
    expect(container.querySelector(".thermal-gauge-grid strong.warning")).toBeNull();
    expect(container.querySelector(".thermal-caveat")?.textContent).toContain(
      "not the CPU die",
    );
  });

  it("colours a die temperature that really is critical", () => {
    const { container } = render(
      <ThermalSection
        kind="cpu"
        thermal={metric({
          cpu: {
            headlineC: 101,
            headlineLabel: "Package id 0",
            caveat: null,
            sensors: [sensor({ temperatureC: 101 })],
          },
        })}
      />,
    );
    expect(container.querySelector(".thermal-gauge-grid strong.critical")).not.toBeNull();
  });
});

describe("ThermalChip", () => {
  it("shows the headline temperature, and nothing at all when unsupported", () => {
    const { container: measured } = render(
      <ThermalChip
        kind="cpu"
        thermal={metric({
          cpu: {
            headlineC: 62,
            headlineLabel: "Package id 0",
            caveat: null,
            sensors: [sensor()],
          },
        })}
      />,
    );
    expect(measured.textContent).toContain("62 C");

    // The bar cards are narrow: a permanent "not supported" would cost a line on
    // every WSL2, container, and macOS host while saying nothing actionable.
    const { container: unsupported } = render(
      <ThermalChip
        kind="cpu"
        thermal={metric({ unsupported: [{ category: "cpu", reason: WSL_REASON }] })}
      />,
    );
    expect(unsupported.textContent).toBe("");
    expect(unsupported.querySelector("svg")).toBeNull();

    const { container: pending } = render(<ThermalChip kind="cpu" thermal={null} />);
    expect(pending.textContent).toContain("n/a");
  });
});
