import { describe, expect, it } from "vitest";
import {
  createTerminalLayout,
  getTerminalLayoutPaneIds,
  insertTerminalPane,
  reconcileTerminalLayout,
  removeTerminalPaneFromLayout,
  updateTerminalSplitRatio,
} from "./terminalLayout";

describe("terminalLayout", () => {
  it("inserts a pane relative to the focused pane with the requested ratio", () => {
    const initial = createTerminalLayout(["one", "two"]);
    const next = insertTerminalPane(initial, "one", "three", "bottom", 35);

    expect(getTerminalLayoutPaneIds(next)).toEqual(["one", "three", "two"]);
    expect(next).toMatchObject({
      type: "split",
      first: {
        type: "split",
        direction: "vertical",
        ratio: 65,
        first: { terminalId: "one" },
        second: { terminalId: "three" },
      },
    });
  });

  it("collapses an empty split and keeps the remaining pane", () => {
    const initial = createTerminalLayout(["one", "two"]);
    const next = removeTerminalPaneFromLayout(initial, "one");

    expect(next).toEqual({ type: "pane", terminalId: "two" });
  });

  it("reconciles backend pane removal without losing the chosen layout", () => {
    let layout = createTerminalLayout(["one"]);
    layout = insertTerminalPane(layout, "one", "two", "bottom", 40);
    layout = reconcileTerminalLayout(layout, ["two", "three"]);

    expect(getTerminalLayoutPaneIds(layout)).toEqual(["two", "three"]);
    expect(layout).toMatchObject({ direction: "horizontal", ratio: 50 });
  });

  it("updates and clamps a nested split ratio", () => {
    let layout = createTerminalLayout(["one"]);
    layout = insertTerminalPane(layout, "one", "two", "right", 50);
    layout = insertTerminalPane(layout, "one", "three", "bottom", 50);

    const next = updateTerminalSplitRatio(layout, ["first"], 4);
    expect(next).toMatchObject({
      first: { type: "split", ratio: 15 },
    });
  });
});
