export type TerminalSplitPlacement = "left" | "right" | "top" | "bottom";

export type TerminalLayoutNode =
  | { type: "pane"; terminalId: string }
  | {
      type: "split";
      direction: "horizontal" | "vertical";
      ratio: number;
      first: TerminalLayoutNode;
      second: TerminalLayoutNode;
    };

const MIN_SPLIT_RATIO = 15;
const MAX_SPLIT_RATIO = 85;

export function clampSplitRatio(ratio: number) {
  return Math.min(MAX_SPLIT_RATIO, Math.max(MIN_SPLIT_RATIO, Math.round(ratio)));
}

export function createTerminalLayout(
  terminalIds: readonly string[],
): TerminalLayoutNode | null {
  const [first, ...remaining] = terminalIds;
  if (!first) {
    return null;
  }

  return remaining.reduce<TerminalLayoutNode>(
    (layout, terminalId) => ({
      type: "split",
      direction: "horizontal",
      ratio: 50,
      first: layout,
      second: { type: "pane", terminalId },
    }),
    { type: "pane", terminalId: first },
  );
}

export function getTerminalLayoutPaneIds(
  layout: TerminalLayoutNode | null,
): string[] {
  if (!layout) {
    return [];
  }
  if (layout.type === "pane") {
    return [layout.terminalId];
  }
  return [
    ...getTerminalLayoutPaneIds(layout.first),
    ...getTerminalLayoutPaneIds(layout.second),
  ];
}

export function insertTerminalPane(
  layout: TerminalLayoutNode | null,
  targetTerminalId: string | null,
  terminalId: string,
  placement: TerminalSplitPlacement,
  newPaneRatio: number,
): TerminalLayoutNode {
  const newPane: TerminalLayoutNode = { type: "pane", terminalId };
  if (!layout) {
    return newPane;
  }

  const direction =
    placement === "left" || placement === "right"
      ? "horizontal"
      : "vertical";
  const newPaneFirst = placement === "left" || placement === "top";
  const clampedNewPaneRatio = clampSplitRatio(newPaneRatio);

  const wrapTarget = (target: TerminalLayoutNode): TerminalLayoutNode => ({
    type: "split",
    direction,
    ratio: newPaneFirst ? clampedNewPaneRatio : 100 - clampedNewPaneRatio,
    first: newPaneFirst ? newPane : target,
    second: newPaneFirst ? target : newPane,
  });

  if (!targetTerminalId) {
    return wrapTarget(layout);
  }

  const insert = (node: TerminalLayoutNode): [TerminalLayoutNode, boolean] => {
    if (node.type === "pane") {
      return node.terminalId === targetTerminalId
        ? [wrapTarget(node), true]
        : [node, false];
    }

    const [first, insertedFirst] = insert(node.first);
    if (insertedFirst) {
      return [{ ...node, first }, true];
    }
    const [second, insertedSecond] = insert(node.second);
    return [{ ...node, second }, insertedSecond];
  };

  const [next, inserted] = insert(layout);
  return inserted ? next : wrapTarget(layout);
}

export function removeTerminalPaneFromLayout(
  layout: TerminalLayoutNode | null,
  terminalId: string,
): TerminalLayoutNode | null {
  if (!layout) {
    return null;
  }
  if (layout.type === "pane") {
    return layout.terminalId === terminalId ? null : layout;
  }

  const first = removeTerminalPaneFromLayout(layout.first, terminalId);
  const second = removeTerminalPaneFromLayout(layout.second, terminalId);
  if (!first) {
    return second;
  }
  if (!second) {
    return first;
  }
  return { ...layout, first, second };
}

export function reconcileTerminalLayout(
  layout: TerminalLayoutNode | null,
  terminalIds: readonly string[],
): TerminalLayoutNode | null {
  const allowed = new Set(terminalIds);
  let next = layout;

  for (const terminalId of getTerminalLayoutPaneIds(layout)) {
    if (!allowed.has(terminalId)) {
      next = removeTerminalPaneFromLayout(next, terminalId);
    }
  }

  const present = new Set(getTerminalLayoutPaneIds(next));
  for (const terminalId of terminalIds) {
    if (!present.has(terminalId)) {
      const targetIds = getTerminalLayoutPaneIds(next);
      next = insertTerminalPane(
        next,
        targetIds[targetIds.length - 1] ?? null,
        terminalId,
        "right",
        50,
      );
      present.add(terminalId);
    }
  }
  return next;
}

export function updateTerminalSplitRatio(
  layout: TerminalLayoutNode | null,
  path: readonly ("first" | "second")[],
  ratio: number,
): TerminalLayoutNode | null {
  if (!layout || layout.type === "pane") {
    return layout;
  }
  if (path.length === 0) {
    return { ...layout, ratio: clampSplitRatio(ratio) };
  }

  const [branch, ...remaining] = path;
  return {
    ...layout,
    [branch]: updateTerminalSplitRatio(layout[branch], remaining, ratio),
  };
}
