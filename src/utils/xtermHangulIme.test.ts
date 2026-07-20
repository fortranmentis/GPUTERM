import { describe, expect, it } from "vitest";
import { computeImeDiff } from "./xtermHangulIme";

// Snapshots below mirror the real WKWebView delivery captured while typing
// 안녕하세요: insertText appends a jamo, insertReplacementText rewrites the
// composing syllable in place, and batchim reflow (녛 → 녀 + 하) arrives as a
// replacement followed by an insert.
describe("computeImeDiff", () => {
  it("appends a fresh syllable-start jamo without deletions", () => {
    expect(computeImeDiff("", "ㅇ")).toEqual({ deleteCount: 0, insert: "ㅇ" });
    expect(computeImeDiff("안", "안ㄴ")).toEqual({ deleteCount: 0, insert: "ㄴ" });
  });

  it("rewrites the composing syllable with one DEL", () => {
    expect(computeImeDiff("ㅇ", "아")).toEqual({ deleteCount: 1, insert: "아" });
    expect(computeImeDiff("아", "안")).toEqual({ deleteCount: 1, insert: "안" });
    expect(computeImeDiff("안ㄴ", "안녀")).toEqual({ deleteCount: 1, insert: "녀" });
  });

  it("handles batchim reflow as replacement then insert", () => {
    // 안녀 + ㅎ → 안녛, then the next vowel pulls ㅎ into a new syllable.
    expect(computeImeDiff("안녀", "안녛")).toEqual({ deleteCount: 1, insert: "녛" });
    expect(computeImeDiff("안녛", "안녀")).toEqual({ deleteCount: 1, insert: "녀" });
    expect(computeImeDiff("안녀", "안녀하")).toEqual({ deleteCount: 0, insert: "하" });
  });

  it("returns an empty diff for identical snapshots", () => {
    expect(computeImeDiff("안녕하세요", "안녕하세요")).toEqual({
      deleteCount: 0,
      insert: "",
    });
  });

  it("counts astral-plane characters as single deletions", () => {
    // Emoji palette insertions ride the same keydown-less insertText path.
    expect(computeImeDiff("😀", "😀🎉")).toEqual({ deleteCount: 0, insert: "🎉" });
    expect(computeImeDiff("a😀", "a")).toEqual({ deleteCount: 1, insert: "" });
  });

  it("rewrites the whole tail when the change is not at the end", () => {
    expect(computeImeDiff("한그", "한글자")).toEqual({
      deleteCount: 1,
      insert: "글자",
    });
  });
});
