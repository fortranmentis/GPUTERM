import type { Terminal } from "@xterm/xterm";

// WKWebView (macOS/Linux WebKit — the engine under Tauri) does not drive IME
// composition events for Korean input inside xterm's hidden textarea. Instead
// each jamo arrives as a bare `insertText` (syllable start) or
// `insertReplacementText` (syllable update: ㅇ→아→안), with keydown
// (keyCode 229) fired AFTER the input event. Stock xterm then leaks raw jamo
// for syllable starts, drops every syllable update, and loses batchim reflow
// characters to its key-rollover gate — Korean comes out shredded.
//
// The workaround intercepts those events ahead of xterm and streams the
// textarea diff terminal-style: one DEL per replaced character followed by
// the rewritten tail (exactly what Terminal.app sends while composing 한).
// Chromium hosts (Windows WebView2) and real composition-event flows
// (desktop Safari) are deliberately left untouched.

export type ImeDiff = {
  deleteCount: number;
  insert: string;
};

/**
 * Backspace-rewrite diff between two textarea snapshots, counted in code
 * points (the remote line editor erases one character per DEL).
 */
export function computeImeDiff(previous: string, next: string): ImeDiff {
  const before = [...previous];
  const after = [...next];
  let common = 0;
  while (
    common < before.length &&
    common < after.length &&
    before[common] === after[common]
  ) {
    common += 1;
  }
  return {
    deleteCount: before.length - common,
    insert: after.slice(common).join(""),
  };
}

const DEL = "\x7f";
const IME_KEYCODE = 229;

function isNonChromiumWebKit(): boolean {
  const agent = navigator.userAgent;
  return (
    agent.includes("AppleWebKit") &&
    !agent.includes("Chrome") &&
    !agent.includes("Chromium")
  );
}

/**
 * Attaches the WebKit Hangul IME workaround to an opened terminal. Returns a
 * disposer; a no-op one on Chromium hosts, where composition works natively.
 */
export function attachWebKitHangulImeWorkaround(terminal: Terminal): {
  dispose(): void;
} {
  const element = terminal.element;
  const textarea = terminal.textarea;
  if (!element || !textarea || !isNonChromiumWebKit()) {
    return { dispose() {} };
  }

  // Snapshot taken at beforeinput, i.e. immediately before WebKit mutates the
  // textarea — robust even if xterm cleared the value in between keystrokes.
  let valueBeforeInput = "";
  // Ordinary printable keys fire a keypress (which xterm already turned into
  // data) right before their input event; IME-delivered text never fires
  // one. Matching the keypress character against the input data separates
  // the two shapes even right after Backspace/Enter, whose stale keycodes
  // would fool a keydown-based rule.
  let lastKeypressKey: string | null = null;

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.target !== textarea) {
      return;
    }
    if (event.keyCode === IME_KEYCODE && !event.isComposing) {
      // Keep xterm's CompositionHelper from running its deferred textarea
      // diff for these keydowns — under this delivery shape it can re-send
      // the whole line. The input handler below owns the data instead.
      event.stopImmediatePropagation();
    }
  };

  const onKeyPress = (event: KeyboardEvent) => {
    if (event.target === textarea) {
      lastKeypressKey = event.key;
    }
  };

  const onBeforeInput = (event: Event) => {
    if (event.target === textarea) {
      valueBeforeInput = textarea.value;
    }
  };

  const onInput = (event: Event) => {
    const inputEvent = event as InputEvent;
    if (inputEvent.target !== textarea) {
      return;
    }
    const ordinaryTyping =
      lastKeypressKey !== null && inputEvent.data === lastKeypressKey;
    lastKeypressKey = null;
    if (
      ordinaryTyping ||
      inputEvent.isComposing ||
      (inputEvent.inputType !== "insertText" &&
        inputEvent.inputType !== "insertReplacementText")
    ) {
      return;
    }
    event.stopImmediatePropagation();
    const { deleteCount, insert } = computeImeDiff(
      valueBeforeInput,
      textarea.value,
    );
    const data = DEL.repeat(deleteCount) + insert;
    if (data.length > 0) {
      terminal.input(data, true);
    }
  };

  // Capture phase on the terminal root runs before xterm's own textarea
  // listeners, so stopImmediatePropagation above can pre-empt them.
  element.addEventListener("keydown", onKeyDown, true);
  element.addEventListener("keypress", onKeyPress, true);
  element.addEventListener("beforeinput", onBeforeInput, true);
  element.addEventListener("input", onInput, true);
  return {
    dispose() {
      element.removeEventListener("keydown", onKeyDown, true);
      element.removeEventListener("keypress", onKeyPress, true);
      element.removeEventListener("beforeinput", onBeforeInput, true);
      element.removeEventListener("input", onInput, true);
    },
  };
}
