export type TerminalShortcutAction =
  | "splitRight"
  | "splitDown"
  | "closePane"
  | "clear"
  | "find"
  | "findNext"
  | "findPrevious"
  | "fontBigger"
  | "fontSmaller"
  | "fontReset"
  | "previousPane"
  | "nextPane";

type KeyLike = Pick<
  KeyboardEvent,
  "key" | "code" | "ctrlKey" | "metaKey" | "shiftKey" | "altKey"
>;

type Binding = {
  action: TerminalShortcutAction;
  label: string;
  /** Physical key (`KeyboardEvent.code`), so Shift never changes the match. */
  code: string;
  shift: boolean;
  /** Font zoom uses plain Ctrl off macOS, as other terminals do. */
  plainCtrl?: boolean;
  /** Matched without any modifier (F3 for find next off macOS). */
  bare?: boolean;
};

export function isMacPlatform(): boolean {
  return typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
}

/*
 * macOS uses Cmd, which the shell never sees. Elsewhere Ctrl+D/W/K are shell
 * keys, so the terminal shortcuts take Ctrl+Shift instead. Cmd+W is left to
 * the window menu, so closing a pane is Cmd+Shift+W.
 */
const MAC_BINDINGS: Binding[] = [
  { action: "splitRight", label: "Split right", code: "KeyD", shift: false },
  { action: "splitDown", label: "Split down", code: "KeyD", shift: true },
  { action: "closePane", label: "Close pane", code: "KeyW", shift: true },
  { action: "clear", label: "Clear screen", code: "KeyK", shift: false },
  { action: "find", label: "Find", code: "KeyF", shift: false },
  { action: "findNext", label: "Find next", code: "KeyG", shift: false },
  { action: "findPrevious", label: "Find previous", code: "KeyG", shift: true },
  { action: "fontBigger", label: "Bigger font", code: "Equal", shift: false },
  { action: "fontSmaller", label: "Smaller font", code: "Minus", shift: false },
  { action: "fontReset", label: "Reset font size", code: "Digit0", shift: false },
  { action: "previousPane", label: "Previous pane", code: "BracketLeft", shift: false },
  { action: "nextPane", label: "Next pane", code: "BracketRight", shift: false },
];

const OTHER_BINDINGS: Binding[] = [
  { action: "splitRight", label: "Split right", code: "KeyD", shift: true },
  { action: "splitDown", label: "Split down", code: "KeyE", shift: true },
  { action: "closePane", label: "Close pane", code: "KeyW", shift: true },
  { action: "clear", label: "Clear screen", code: "KeyK", shift: true },
  { action: "find", label: "Find", code: "KeyF", shift: true },
  { action: "findNext", label: "Find next", code: "F3", shift: false, bare: true },
  { action: "findPrevious", label: "Find previous", code: "F3", shift: true, bare: true },
  { action: "fontBigger", label: "Bigger font", code: "Equal", shift: false, plainCtrl: true },
  { action: "fontSmaller", label: "Smaller font", code: "Minus", shift: false, plainCtrl: true },
  { action: "fontReset", label: "Reset font size", code: "Digit0", shift: false, plainCtrl: true },
  { action: "previousPane", label: "Previous pane", code: "BracketLeft", shift: true },
  { action: "nextPane", label: "Next pane", code: "BracketRight", shift: true },
];

export function matchTerminalShortcut(
  event: KeyLike,
  mac = isMacPlatform(),
): TerminalShortcutAction | null {
  if (event.altKey) {
    return null;
  }
  for (const binding of mac ? MAC_BINDINGS : OTHER_BINDINGS) {
    if (event.code !== binding.code) {
      continue;
    }
    if (binding.bare) {
      if (!event.ctrlKey && !event.metaKey && event.shiftKey === binding.shift) {
        return binding.action;
      }
      continue;
    }
    const modifierHeld = mac
      ? event.metaKey && !event.ctrlKey
      : event.ctrlKey && !event.metaKey;
    if (!modifierHeld) {
      continue;
    }
    // Font zoom accepts Shift too, since "+" is Shift+= on most layouts.
    if (binding.plainCtrl || (mac && binding.code === "Equal")) {
      return binding.action;
    }
    if (event.shiftKey === binding.shift) {
      return binding.action;
    }
  }
  return null;
}

const CODE_LABELS: Record<string, string> = {
  Equal: "=",
  Minus: "-",
  Digit0: "0",
  BracketLeft: "[",
  BracketRight: "]",
  F3: "F3",
};

/** `[label, keys]` pairs for the shortcut list in the appearance popover. */
export function shortcutLabels(mac = isMacPlatform()): [string, string][] {
  return (mac ? MAC_BINDINGS : OTHER_BINDINGS).map((binding) => {
    const key = CODE_LABELS[binding.code] ?? binding.code.replace(/^Key/, "");
    const parts: string[] = [];
    if (!binding.bare) {
      parts.push(mac ? "⌘" : "Ctrl");
    }
    if (binding.shift) {
      parts.push(mac ? "⇧" : "Shift");
    }
    parts.push(key);
    return [binding.label, parts.join(mac ? "" : "+")];
  });
}
