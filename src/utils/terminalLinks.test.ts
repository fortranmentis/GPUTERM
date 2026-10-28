import { describe, expect, it } from "vitest";
import { findUrls, rangeFor, tunnelTarget } from "./terminalLinks";

describe("terminal links", () => {
  it("finds URLs and drops trailing punctuation", () => {
    const text = "Open http://localhost:6006/ (TensorBoard), or see https://example.com/docs.";
    expect(findUrls(text).map((found) => found.url)).toEqual([
      "http://localhost:6006/",
      "https://example.com/docs",
    ]);
    const [first] = findUrls(text);
    expect(text.slice(first.start, first.end)).toBe("http://localhost:6006/");
  });

  it("tunnels loopback and own-host http links with their token", () => {
    expect(tunnelTarget("http://127.0.0.1:8888/lab?token=abc", [])).toEqual({
      host: "127.0.0.1",
      port: 8888,
      path: "/lab?token=abc",
    });
    expect(tunnelTarget("http://0.0.0.0:6006/", [])?.port).toBe(6006);
    expect(tunnelTarget("http://[::1]:8000/", [])?.host).toBe("::1");
    expect(tunnelTarget("http://gpu-02:8888/tree", ["gpu-02.lab.example.org"])).toEqual({
      host: "localhost",
      port: 8888,
      path: "/tree",
    });
  });

  it("leaves other links alone", () => {
    expect(tunnelTarget("https://localhost:8443/", [])).toBeNull();
    expect(tunnelTarget("http://example.com:8080/", ["gpu-02"])).toBeNull();
    expect(tunnelTarget("not a url", [])).toBeNull();
  });

  it("maps offsets in a wrapped line to buffer cells", () => {
    expect(rangeFor(5, 15, 10, 80)).toEqual({ start: { x: 6, y: 11 }, end: { x: 15, y: 11 } });
    expect(rangeFor(70, 95, 10, 80)).toEqual({ start: { x: 71, y: 11 }, end: { x: 15, y: 12 } });
  });
});
