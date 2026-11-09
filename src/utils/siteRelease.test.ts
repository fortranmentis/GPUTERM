import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";
import { waitFor } from "@testing-library/react";

const script = readFileSync("site/assets/site.js", "utf8");

describe("website release selection", () => {
  afterEach(() => { document.body.innerHTML = ""; });

  async function run(releases: unknown[]) {
    document.body.innerHTML = '<a data-download="windows" href="/releases">Download</a><span data-release-version></span>';
    const fetch = vi.fn().mockResolvedValue({ ok: true, json: async () => releases });
    new Function("window", "document", "navigator", "fetch", script)(
      {}, document, { platform: "", userAgent: "" }, fetch,
    );
    await waitFor(() => expect(fetch).toHaveBeenCalledOnce());
  }
  const release = (tag: string, prerelease: boolean) => ({
    tag_name: tag, prerelease, draft: false,
    html_url: `https://example.test/${tag}`,
    assets: [{ name: "GpuTerm_x64-setup.exe", browser_download_url: `https://example.test/${tag}/setup` }],
  });

  it("prefers the official release over a newer beta and updater infrastructure", async () => {
    await run([release("updater", true), release("v1.5.0-beta", true), release("v1.4.0", false)]);
    await waitFor(() => expect(document.querySelector("[data-release-version]")).toHaveTextContent("1.4.0"));
    expect(document.querySelector("a")).toHaveAttribute("href", "https://example.test/v1.4.0/setup");
  });
  it("retains a beta fallback when there are no official releases", async () => {
    await run([release("v1.3.9-beta", true)]);
    await waitFor(() => expect(document.querySelector("[data-release-version]")).toHaveTextContent("1.3.9-beta"));
  });
  it("does not offer drafts or infrastructure releases", async () => {
    await run([release("updater", true), { ...release("v2.0.0", false), draft: true }]);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(document.querySelector("a")).toHaveAttribute("href", "/releases");
  });
});
