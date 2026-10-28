import { describe, expect, it } from "vitest";
import { buildManifest } from "./make-updater-manifest.mjs";

const signatures = {
  "GpuTerm_1.3.8-beta_aarch64.app.tar.gz": "mac-sig\n",
  "GpuTerm_1.3.8-beta_x64-setup.exe": "win-sig\n",
  "GpuTerm_1.3.8-beta_amd64.AppImage": "linux-sig\n",
};

describe("buildManifest", () => {
  it("lists one signed bundle per platform", () => {
    const manifest = buildManifest({
      tag: "v1.3.8-beta",
      repo: "fortranmentis/GPUTERM",
      notes: "# GpuTerm 1.3.8-beta\n\nNotes\n",
      pubDate: "2026-10-28T00:00:00.000Z",
      signatures,
    });
    expect(manifest).toEqual({
      version: "1.3.8-beta",
      notes: "# GpuTerm 1.3.8-beta\n\nNotes",
      pub_date: "2026-10-28T00:00:00.000Z",
      platforms: {
        "darwin-aarch64": {
          signature: "mac-sig",
          url: "https://github.com/fortranmentis/GPUTERM/releases/download/v1.3.8-beta/GpuTerm_1.3.8-beta_aarch64.app.tar.gz",
        },
        "windows-x86_64": {
          signature: "win-sig",
          url: "https://github.com/fortranmentis/GPUTERM/releases/download/v1.3.8-beta/GpuTerm_1.3.8-beta_x64-setup.exe",
        },
        "linux-x86_64": {
          signature: "linux-sig",
          url: "https://github.com/fortranmentis/GPUTERM/releases/download/v1.3.8-beta/GpuTerm_1.3.8-beta_amd64.AppImage",
        },
      },
    });
  });

  it("refuses a release with a missing or duplicate bundle", () => {
    const base = { tag: "v1", repo: "o/r", notes: "", pubDate: "" };
    const withoutLinux = Object.fromEntries(
      Object.entries(signatures).filter(([name]) => !name.endsWith(".AppImage")),
    );
    expect(() => buildManifest({ ...base, signatures: withoutLinux })).toThrow(
      /linux-x86_64 update bundle, found 0/,
    );
    expect(() =>
      buildManifest({ ...base, signatures: { ...signatures, "Other_x64-setup.exe": "x" } }),
    ).toThrow(/windows-x86_64 update bundle, found 2/);
  });
});
