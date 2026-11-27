import { describe, expect, it } from "vitest";
import { cellOffsetFor, findUrls, rangeFor, rangeForText, tunnelTarget } from "./terminalLinks";

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

  it("accounts for wide Hangul, CJK, and emoji cells before a URL", () => {
    // "접속: " = 2 + 2 + 1 + 1 = 6 display cells (vs 4 UTF-16 code units).
    const hangulLine = "접속: http://localhost:6006/";
    const [hangulMatch] = findUrls(hangulLine);
    expect(hangulMatch.start).toBe(4);
    expect(cellOffsetFor(hangulLine, hangulMatch.start, 80)).toBe(6);
    expect(rangeForText(hangulLine, hangulMatch.start, hangulMatch.end, 0, 80)).toEqual({
      start: { x: 7, y: 1 },
      end: { x: 28, y: 1 },
    });

    // "🔥中文 http://example.com" = 2 (emoji) + 2 + 2 + 1 = 7 display cells (vs 5 UTF-16 units).
    const mixedLine = "🔥中文 http://example.com";
    const [mixedMatch] = findUrls(mixedLine);
    expect(mixedMatch.start).toBe(5);
    expect(cellOffsetFor(mixedLine, mixedMatch.start, 80)).toBe(7);
    expect(rangeForText(mixedLine, mixedMatch.start, mixedMatch.end, 2, 80)).toEqual({
      start: { x: 8, y: 3 },
      end: { x: 25, y: 3 },
    });

    // Wide char at the last column wraps to the next row, leaving 1 padding cell.
    // Cols = 5: "abcd" (4 cells) + "한" (wraps to col 0 of next row -> cells 5..6) + " " (cell 7)
    const wrapped = "abcd한 http://a.io";
    const [wrappedMatch] = findUrls(wrapped);
    expect(cellOffsetFor(wrapped, wrappedMatch.start, 5)).toBe(8);
  });
});
