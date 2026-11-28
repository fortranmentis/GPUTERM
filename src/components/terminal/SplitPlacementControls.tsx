import { PanelBottom, PanelLeft, PanelRight, PanelTop } from "lucide-react";
import type { TerminalSplitPlacement } from "../../utils/terminalLayout";

type SplitPlacementControlsProps = {
  placement: TerminalSplitPlacement;
  ratio: number;
  onPlacementChange: (placement: TerminalSplitPlacement) => void;
  onRatioChange: (ratio: number) => void;
};

export function SplitPlacementControls({
  placement,
  ratio,
  onPlacementChange,
  onRatioChange,
}: SplitPlacementControlsProps) {
  const placements: Array<{
    value: TerminalSplitPlacement;
    label: string;
    icon: typeof PanelLeft;
  }> = [
    { value: "left", label: "Left", icon: PanelLeft },
    { value: "right", label: "Right", icon: PanelRight },
    { value: "top", label: "Top", icon: PanelTop },
    { value: "bottom", label: "Bottom", icon: PanelBottom },
  ];

  return (
    <div className="terminal-split-options">
      <fieldset>
        <legend>New pane position</legend>
        <div className="terminal-split-placement-grid">
          {placements.map((option) => {
            const Icon = option.icon;
            return (
              <button
                key={option.value}
                type="button"
                className={placement === option.value ? "selected" : ""}
                aria-pressed={placement === option.value}
                aria-label={`Place new pane ${option.label.toLowerCase()}`}
                onClick={() => onPlacementChange(option.value)}
              >
                <Icon size={15} />
                {option.label}
              </button>
            );
          })}
        </div>
      </fieldset>
      <label className="terminal-split-ratio">
        <span>
          New pane size <output>{ratio}%</output>
        </span>
        <input
          type="range"
          min="20"
          max="80"
          step="5"
          value={ratio}
          aria-label="New pane size"
          onChange={(event) => onRatioChange(Number(event.target.value))}
        />
      </label>
    </div>
  );
}
