/**
 * Multi-selection rules shared by the SFTP remote and local file panels.
 *
 * `paths` is the selection; `anchor` is the last entry clicked without Shift,
 * which a later Shift+click extends a range from. `ordered` is always the
 * panel's current display order (after sorting), so ranges follow what the
 * user sees.
 */
export type FileSelection = {
  paths: string[];
  anchor: string | null;
};

export const EMPTY_SELECTION: FileSelection = { paths: [], anchor: null };

export type SelectionGesture = {
  /** Shift held: select the range from the anchor. */
  shift: boolean;
  /** Ctrl/Cmd held: toggle, or union a Shift range with the selection. */
  additive: boolean;
  /** The click landed on the row's checkbox, which always toggles. */
  checkbox?: boolean;
};

export type SelectAllState = "none" | "some" | "all";

/** Adds or removes one path and makes it the anchor. */
export function toggle(selection: FileSelection, path: string): FileSelection {
  const paths = selection.paths.includes(path)
    ? selection.paths.filter((item) => item !== path)
    : [...selection.paths, path];
  return { paths, anchor: path };
}

/** Selects only `path`. */
export function selectSingle(path: string): FileSelection {
  return { paths: [path], anchor: path };
}

/**
 * Selects every path between the anchor and `path` in display order. With
 * `additive` the range joins the current selection, otherwise it replaces it.
 * The anchor stays put so repeated Shift+clicks pivot around it; without an
 * anchor (or one no longer listed) the range is just `path`.
 */
export function selectRange(
  selection: FileSelection,
  ordered: string[],
  path: string,
  additive: boolean,
): FileSelection {
  const target = ordered.indexOf(path);
  if (target < 0) {
    return selection;
  }
  const anchorIndex =
    selection.anchor === null ? -1 : ordered.indexOf(selection.anchor);
  const start = anchorIndex < 0 ? target : Math.min(anchorIndex, target);
  const end = anchorIndex < 0 ? target : Math.max(anchorIndex, target);
  const range = ordered.slice(start, end + 1);
  const paths = additive
    ? [...selection.paths, ...range.filter((item) => !selection.paths.includes(item))]
    : range;
  return { paths, anchor: anchorIndex < 0 ? path : selection.anchor };
}

/** Header checkbox: selects everything unless everything is already selected. */
export function toggleAll(selection: FileSelection, ordered: string[]): FileSelection {
  if (ordered.length > 0 && selectAllState(selection.paths, ordered) === "all") {
    return EMPTY_SELECTION;
  }
  return { paths: [...ordered], anchor: selection.anchor };
}

/** Selects every listed path (Ctrl/Cmd+A). */
export function selectAll(selection: FileSelection, ordered: string[]): FileSelection {
  return { paths: [...ordered], anchor: selection.anchor };
}

export function selectAllState(selected: string[], ordered: string[]): SelectAllState {
  const count = ordered.filter((path) => selected.includes(path)).length;
  if (count === 0) {
    return "none";
  }
  return count === ordered.length ? "all" : "some";
}

/**
 * One click on a row or its checkbox. A plain row click keeps single-select
 * (an already sole-selected row is left as is); Ctrl/Cmd-click and the
 * checkbox toggle; Shift selects a range from the anchor.
 */
export function applySelectionGesture(
  selection: FileSelection,
  ordered: string[],
  path: string,
  gesture: SelectionGesture,
): FileSelection {
  if (gesture.shift) {
    return selectRange(selection, ordered, path, gesture.additive);
  }
  if (gesture.checkbox || gesture.additive) {
    return toggle(selection, path);
  }
  if (selection.paths.length === 1 && selection.paths[0] === path) {
    return { paths: selection.paths, anchor: path };
  }
  return selectSingle(path);
}

/** Bulk action label: `Download (3)` once more than one item is selected. */
export function countLabel(label: string, count: number): string {
  return count > 1 ? `${label} (${count})` : label;
}

/** Whether Ctrl/Cmd+A should select files rather than text in a field. */
export function isTextEditingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  if (target.isContentEditable) {
    return true;
  }
  if (target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) {
    return true;
  }
  if (target instanceof HTMLInputElement) {
    return !["checkbox", "radio", "button", "submit", "reset"].includes(target.type);
  }
  return false;
}
