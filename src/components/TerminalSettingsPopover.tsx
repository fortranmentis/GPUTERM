import { useEffect, useRef, useState } from "react";
import { Minus, Palette, Plus } from "lucide-react";
import { useSettingsStore } from "../stores/settingsStore";
import {
  DEFAULT_TERMINAL_FONT_SIZE,
  TERMINAL_FONTS,
  TERMINAL_THEMES,
} from "../utils/terminalAppearance";
import { shortcutLabels } from "../utils/terminalShortcuts";

/** Toolbar popover for the terminal theme, font and size. */
export function TerminalSettingsPopover() {
  const anchorRef = useRef<HTMLDivElement | null>(null);
  const [open, setOpen] = useState(false);
  const appearance = useSettingsStore((state) => state.terminalAppearance);
  const setAppearance = useSettingsStore((state) => state.setTerminalAppearance);

  useEffect(() => {
    if (!open) {
      return;
    }
    const handlePointerDown = (event: MouseEvent) => {
      if (!anchorRef.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [open]);

  return (
    <div className="terminal-tmux-picker-anchor" ref={anchorRef}>
      <button
        className="icon-button"
        type="button"
        aria-label="Terminal appearance"
        title="Theme, font and shortcuts"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        <Palette size={16} />
      </button>
      {open && (
        <div
          className="terminal-tmux-picker terminal-appearance-picker"
          role="dialog"
          aria-label="Terminal appearance"
        >
          <div className="terminal-session-picker-heading">Appearance</div>
          <div className="terminal-theme-grid" role="radiogroup" aria-label="Theme">
            {TERMINAL_THEMES.map((theme) => (
              <button
                key={theme.id}
                type="button"
                role="radio"
                aria-checked={theme.id === appearance.theme.id}
                className={theme.id === appearance.theme.id ? "selected" : undefined}
                style={{
                  background: theme.xterm.background,
                  color: theme.xterm.foreground,
                }}
                onClick={() => setAppearance((current) => ({ ...current, theme }))}
              >
                <span className="terminal-theme-swatches" aria-hidden="true">
                  {[theme.xterm.red, theme.xterm.green, theme.xterm.yellow, theme.xterm.blue].map(
                    (color, index) => (
                      <i key={index} style={{ background: color }} />
                    ),
                  )}
                </span>
                {theme.name}
              </button>
            ))}
          </div>
          <label>
            <span>Font</span>
            <select
              value={appearance.font.id}
              onChange={(event) => {
                const font = TERMINAL_FONTS.find((item) => item.id === event.target.value);
                if (font) {
                  setAppearance((current) => ({ ...current, font }));
                }
              }}
            >
              {TERMINAL_FONTS.map((font) => (
                <option key={font.id} value={font.id}>
                  {font.name}
                </option>
              ))}
            </select>
          </label>
          <div className="terminal-font-size-row">
            <span>Size</span>
            <button
              className="icon-button ghost"
              type="button"
              aria-label="Smaller font"
              onClick={() =>
                setAppearance((current) => ({ ...current, fontSize: current.fontSize - 1 }))
              }
            >
              <Minus size={14} />
            </button>
            <output aria-live="polite">{appearance.fontSize}px</output>
            <button
              className="icon-button ghost"
              type="button"
              aria-label="Bigger font"
              onClick={() =>
                setAppearance((current) => ({ ...current, fontSize: current.fontSize + 1 }))
              }
            >
              <Plus size={14} />
            </button>
            <button
              className="secondary-button compact"
              type="button"
              disabled={appearance.fontSize === DEFAULT_TERMINAL_FONT_SIZE}
              onClick={() =>
                setAppearance((current) => ({
                  ...current,
                  fontSize: DEFAULT_TERMINAL_FONT_SIZE,
                }))
              }
            >
              Reset
            </button>
          </div>
          <div className="terminal-session-picker-heading">Shortcuts</div>
          <dl className="terminal-shortcut-list">
            {shortcutLabels().map(([label, keys]) => (
              <div key={label}>
                <dt>{label}</dt>
                <dd>{keys}</dd>
              </div>
            ))}
          </dl>
        </div>
      )}
    </div>
  );
}
