import { describe, expect, it } from "vitest";
import { formatBytes, formatFileSize } from "./formatBytes";

describe("byte formatters", () => {
  it("keeps resource telemetry on binary IEC units", () => {
    expect(formatBytes(1024)).toBe("1.00 KiB");
    expect(formatBytes(1024 ** 3)).toBe("1.00 GiB");
  });

  it("formats SFTP file sizes with decimal SI units", () => {
    expect(formatFileSize(null)).toBe("n/a");
    expect(formatFileSize(999)).toBe("999 B");
    expect(formatFileSize(1000)).toBe("1.00 KB");
    expect(formatFileSize(10_000)).toBe("10.0 KB");
    expect(formatFileSize(1_000_000)).toBe("1.00 MB");
    expect(formatFileSize(1_000_000_000)).toBe("1.00 GB");
    expect(formatFileSize(1_000_000_000_000)).toBe("1.00 TB");
  });
});
