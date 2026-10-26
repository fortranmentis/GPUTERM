import { describe, expect, it } from "vitest";
import {
  EMPTY_SELECTION,
  applySelectionGesture,
  countLabel,
  isTextEditingTarget,
  selectAll,
  selectAllState,
  selectRange,
  toggle,
  toggleAll,
} from "./fileSelection";

const ordered = ["/a", "/b", "/c", "/d", "/e"];

describe("fileSelection", () => {
  it("toggles one path without touching the others and moves the anchor", () => {
    const first = toggle(EMPTY_SELECTION, "/b");
    expect(first).toEqual({ paths: ["/b"], anchor: "/b" });
    const second = toggle(first, "/d");
    expect(second).toEqual({ paths: ["/b", "/d"], anchor: "/d" });
    expect(toggle(second, "/b")).toEqual({ paths: ["/d"], anchor: "/b" });
  });

  it("selects a range from the anchor in display order, both directions", () => {
    const anchored = { paths: ["/b"], anchor: "/b" };
    expect(selectRange(anchored, ordered, "/d", false)).toEqual({
      paths: ["/b", "/c", "/d"],
      anchor: "/b",
    });
    expect(selectRange({ paths: ["/d"], anchor: "/d" }, ordered, "/a", false).paths).toEqual([
      "/a",
      "/b",
      "/c",
      "/d",
    ]);
  });

  it("replaces the selection with a range unless Ctrl/Cmd is also held", () => {
    const selection = { paths: ["/e", "/b"], anchor: "/b" };
    expect(selectRange(selection, ordered, "/c", false).paths).toEqual(["/b", "/c"]);
    expect(selectRange(selection, ordered, "/c", true).paths).toEqual(["/e", "/b", "/c"]);
  });

  it("treats a range without an anchor as the clicked path", () => {
    expect(selectRange(EMPTY_SELECTION, ordered, "/c", false)).toEqual({
      paths: ["/c"],
      anchor: "/c",
    });
    // An anchor from a directory that is no longer listed behaves the same.
    expect(selectRange({ paths: [], anchor: "/gone" }, ordered, "/c", false).paths).toEqual([
      "/c",
    ]);
    expect(selectRange(EMPTY_SELECTION, ordered, "/missing", false)).toBe(EMPTY_SELECTION);
  });

  it("selects all, then clears from the header checkbox", () => {
    const all = toggleAll(EMPTY_SELECTION, ordered);
    expect(all.paths).toEqual(ordered);
    expect(toggleAll(all, ordered)).toEqual(EMPTY_SELECTION);
    expect(toggleAll({ paths: ["/a"], anchor: "/a" }, ordered).paths).toEqual(ordered);
    expect(toggleAll(EMPTY_SELECTION, [])).toEqual(EMPTY_SELECTION);
    expect(selectAll({ paths: ["/a"], anchor: "/a" }, ordered)).toEqual({
      paths: ordered,
      anchor: "/a",
    });
  });

  it("reports the tri-state of the header checkbox", () => {
    expect(selectAllState([], ordered)).toBe("none");
    expect(selectAllState(["/a", "/elsewhere"], ordered)).toBe("some");
    expect(selectAllState([...ordered], ordered)).toBe("all");
    expect(selectAllState([], [])).toBe("none");
  });

  it("maps clicks to single select, toggle and range", () => {
    const selection = { paths: ["/a", "/b"], anchor: "/b" };
    expect(applySelectionGesture(selection, ordered, "/c", { shift: false, additive: false })).toEqual(
      { paths: ["/c"], anchor: "/c" },
    );
    expect(
      applySelectionGesture(selection, ordered, "/c", {
        shift: false,
        additive: false,
        checkbox: true,
      }).paths,
    ).toEqual(["/a", "/b", "/c"]);
    expect(
      applySelectionGesture(selection, ordered, "/a", { shift: false, additive: true }).paths,
    ).toEqual(["/b"]);
    expect(
      applySelectionGesture(selection, ordered, "/d", { shift: true, additive: false }).paths,
    ).toEqual(["/b", "/c", "/d"]);
    const sole = { paths: ["/c"], anchor: "/a" };
    expect(applySelectionGesture(sole, ordered, "/c", { shift: false, additive: false })).toEqual({
      paths: ["/c"],
      anchor: "/c",
    });
  });

  it("labels bulk buttons with a count only for several items", () => {
    expect(countLabel("Download", 0)).toBe("Download");
    expect(countLabel("Download", 1)).toBe("Download");
    expect(countLabel("Delete", 3)).toBe("Delete (3)");
  });

  it("leaves Ctrl/Cmd+A to text fields", () => {
    const text = document.createElement("input");
    const checkbox = document.createElement("input");
    checkbox.type = "checkbox";
    expect(isTextEditingTarget(text)).toBe(true);
    expect(isTextEditingTarget(document.createElement("textarea"))).toBe(true);
    expect(isTextEditingTarget(checkbox)).toBe(false);
    expect(isTextEditingTarget(document.createElement("button"))).toBe(false);
    expect(isTextEditingTarget(null)).toBe(false);
  });
});
