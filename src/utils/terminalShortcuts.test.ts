import { describe, expect, it } from "vitest";
import { matchTerminalShortcut, shortcutLabels } from "./terminalShortcuts";

const key = (
  code: string,
  modifiers: Partial<Record<"ctrlKey" | "metaKey" | "shiftKey" | "altKey", boolean>> = {},
) => ({
  key: "",
  code,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
  altKey: false,
  ...modifiers,
});

describe("matchTerminalShortcut", () => {
  it("uses Cmd on macOS", () => {
    expect(matchTerminalShortcut(key("KeyD", { metaKey: true }), true)).toBe("splitRight");
    expect(matchTerminalShortcut(key("KeyD", { metaKey: true, shiftKey: true }), true)).toBe(
      "splitDown",
    );
    expect(matchTerminalShortcut(key("Equal", { metaKey: true, shiftKey: true }), true)).toBe(
      "fontBigger",
    );
    // Cmd+W stays with the window menu.
    expect(matchTerminalShortcut(key("KeyW", { metaKey: true }), true)).toBeNull();
    // Ctrl+D is the shell's EOF on macOS too.
    expect(matchTerminalShortcut(key("KeyD", { ctrlKey: true }), true)).toBeNull();
  });

  it("leaves plain Ctrl shell keys alone elsewhere", () => {
    expect(matchTerminalShortcut(key("KeyD", { ctrlKey: true }), false)).toBeNull();
    expect(matchTerminalShortcut(key("KeyK", { ctrlKey: true }), false)).toBeNull();
    expect(matchTerminalShortcut(key("KeyD", { ctrlKey: true, shiftKey: true }), false)).toBe(
      "splitRight",
    );
    expect(matchTerminalShortcut(key("Equal", { ctrlKey: true }), false)).toBe("fontBigger");
    expect(matchTerminalShortcut(key("F3", { shiftKey: true }), false)).toBe("findPrevious");
    expect(matchTerminalShortcut(key("KeyD", { ctrlKey: true, altKey: true }), false)).toBeNull();
  });

  it("labels every binding", () => {
    expect(shortcutLabels(true)).toContainEqual(["Split down", "⌘⇧D"]);
    expect(shortcutLabels(false)).toContainEqual(["Split right", "Ctrl+Shift+D"]);
  });
});
