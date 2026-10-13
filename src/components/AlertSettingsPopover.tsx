import { invoke } from "@tauri-apps/api/core";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { Bell } from "lucide-react";
import { useEffect, useState, type RefObject } from "react";
import { ResourceDetailPopover } from "./ResourceDetailPopover";
import { useAlertStore } from "../stores/alertStore";
import type { AlertSettings, ThresholdRule } from "../types/alerts";

type RuleKey = "gpuTemp" | "gpuVram" | "memory" | "disk";

const RULES: Array<{ key: RuleKey; label: string; unit: string }> = [
  { key: "gpuTemp", label: "GPU temperature", unit: "°C" },
  { key: "gpuVram", label: "GPU VRAM", unit: "%" },
  { key: "memory", label: "Memory", unit: "%" },
  { key: "disk", label: "Disk (each mount)", unit: "%" },
];

type Permission = "granted" | "denied" | "unknown";

function NumberInput({
  label,
  value,
  onChange,
  min,
  max,
}: {
  label: string;
  value: number;
  onChange: (value: number) => void;
  min?: number;
  max?: number;
}) {
  return (
    <input
      type="number"
      aria-label={label}
      value={Number.isFinite(value) ? value : ""}
      min={min}
      max={max}
      onChange={(event) => onChange(event.target.valueAsNumber)}
    />
  );
}

/** Problems that would make a rule never fire or never clear. */
export function alertSettingsProblem(settings: AlertSettings): string | null {
  for (const { key, label } of RULES) {
    const rule = settings[key];
    if (!Number.isFinite(rule.threshold) || !Number.isFinite(rule.hysteresis)) {
      return `${label}: enter a number`;
    }
    if (rule.hysteresis < 0) {
      return `${label}: "clears below" must not be above the alert level`;
    }
  }
  if (!Number.isFinite(settings.cooldownSecs) || settings.cooldownSecs < 60) {
    return "Wait at least 1 minute before repeating an alert";
  }
  return null;
}

export function AlertSettingsPopover({
  anchorRef,
  onClose,
}: {
  anchorRef: RefObject<HTMLElement | null>;
  onClose: () => void;
}) {
  const saved = useAlertStore((state) => state.settings);
  const setSaved = useAlertStore((state) => state.setSettings);
  const recent = useAlertStore((state) => state.recent);
  const [draft, setDraft] = useState<AlertSettings | null>(saved);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [permission, setPermission] = useState<Permission>("unknown");

  useEffect(() => {
    if (!draft && saved) {
      setDraft(saved);
    }
  }, [draft, saved]);

  useEffect(() => {
    let disposed = false;
    isPermissionGranted()
      .then((granted) => !disposed && setPermission(granted ? "granted" : "denied"))
      .catch(() => undefined);
    return () => {
      disposed = true;
    };
  }, []);

  const update = (patch: Partial<AlertSettings>) =>
    setDraft((current) => (current ? { ...current, ...patch } : current));
  const updateRule = (key: RuleKey, patch: Partial<ThresholdRule>) =>
    setDraft((current) =>
      current ? { ...current, [key]: { ...current[key], ...patch } } : current,
    );

  const problem = draft ? alertSettingsProblem(draft) : null;
  const dirty = draft != null && JSON.stringify(draft) !== JSON.stringify(saved);

  const save = async () => {
    if (!draft || problem) {
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const stored = await invoke<AlertSettings>("update_alert_settings", { settings: draft });
      setSaved(stored);
      setDraft(stored);
    } catch (saveError) {
      setError(String(saveError));
    } finally {
      setSaving(false);
    }
  };

  const allow = async () => {
    try {
      setPermission((await requestPermission()) === "granted" ? "granted" : "denied");
    } catch (permissionError) {
      setError(String(permissionError));
    }
  };

  const test = () => {
    try {
      sendNotification({ title: "GpuTerm", body: "Alerts will look like this." });
    } catch (testError) {
      setError(String(testError));
    }
  };

  return (
    <ResourceDetailPopover
      anchorRef={anchorRef}
      ariaLabel="Alert settings"
      title="Alerts"
      icon={<Bell size={16} />}
      className="alert-settings-popover"
      onClose={onClose}
    >
      {!draft ? (
        <div className="empty-list">Loading alert settings…</div>
      ) : (
        <div className="alert-settings">
          <label className="alert-toggle">
            <input
              type="checkbox"
              checked={draft.enabled}
              onChange={(event) => update({ enabled: event.target.checked })}
            />
            <span>Watch for problems on connected servers</span>
          </label>

          <fieldset disabled={!draft.enabled}>
            <div className="alert-rule-table" role="table" aria-label="Alert rules">
              <div className="alert-rule-row head" role="row">
                <span role="columnheader">Rule</span>
                <span role="columnheader">Alert at</span>
                <span role="columnheader">Clears below</span>
                <span role="columnheader">For (s)</span>
              </div>
              {RULES.map(({ key, label, unit }) => {
                const rule = draft[key];
                return (
                  <div className="alert-rule-row" role="row" key={key}>
                    <label>
                      <input
                        type="checkbox"
                        checked={rule.enabled}
                        onChange={(event) => updateRule(key, { enabled: event.target.checked })}
                      />
                      <span>{label}</span>
                    </label>
                    <span className="alert-number">
                      <NumberInput
                        label={`${label} alert level`}
                        value={rule.threshold}
                        onChange={(threshold) =>
                          // Keep "clears below" where it was.
                          updateRule(key, {
                            threshold,
                            hysteresis: threshold - (rule.threshold - rule.hysteresis),
                          })
                        }
                      />
                      {unit}
                    </span>
                    <span className="alert-number">
                      <NumberInput
                        label={`${label} clears below`}
                        value={rule.threshold - rule.hysteresis}
                        onChange={(clearAt) =>
                          updateRule(key, { hysteresis: rule.threshold - clearAt })
                        }
                      />
                      {unit}
                    </span>
                    <NumberInput
                      label={`${label} sustain seconds`}
                      value={rule.sustainSecs}
                      min={0}
                      onChange={(sustainSecs) => updateRule(key, { sustainSecs })}
                    />
                  </div>
                );
              })}
            </div>

            <label className="alert-toggle">
              <input
                type="checkbox"
                checked={draft.gpuIdle.enabled}
                onChange={(event) =>
                  update({ gpuIdle: { ...draft.gpuIdle, enabled: event.target.checked } })
                }
              />
              <span>
                A GPU sits below{" "}
                <NumberInput
                  label="Idle GPU utilization below"
                  value={draft.gpuIdle.utilBelow}
                  onChange={(utilBelow) => update({ gpuIdle: { ...draft.gpuIdle, utilBelow } })}
                />
                % use while holding{" "}
                <NumberInput
                  label="Idle GPU VRAM at least"
                  value={draft.gpuIdle.minVramPercent}
                  onChange={(minVramPercent) =>
                    update({ gpuIdle: { ...draft.gpuIdle, minVramPercent } })
                  }
                />
                % VRAM for{" "}
                <NumberInput
                  label="Idle GPU minutes"
                  value={Math.round(draft.gpuIdle.sustainSecs / 60)}
                  min={1}
                  onChange={(minutes) =>
                    update({ gpuIdle: { ...draft.gpuIdle, sustainSecs: minutes * 60 } })
                  }
                />{" "}
                min
              </span>
            </label>

            <label className="alert-toggle">
              <input
                type="checkbox"
                checked={draft.llmCritical}
                onChange={(event) => update({ llmCritical: event.target.checked })}
              />
              <span>An LLM runtime turns critical</span>
            </label>

            <label className="alert-toggle">
              <span>
                Repeat the same alert at most every{" "}
                <NumberInput
                  label="Cooldown minutes"
                  value={Math.round(draft.cooldownSecs / 60)}
                  min={1}
                  onChange={(minutes) => update({ cooldownSecs: minutes * 60 })}
                />{" "}
                min
              </span>
            </label>
          </fieldset>

          <div className="alert-notifications">
            <label className="alert-toggle">
              <input
                type="checkbox"
                checked={draft.osNotifications}
                onChange={(event) => update({ osNotifications: event.target.checked })}
              />
              <span>System notifications</span>
            </label>
            {draft.osNotifications && permission === "denied" && (
              <span className="alert-permission">
                Not allowed yet.{" "}
                <button type="button" className="secondary-button compact" onClick={() => void allow()}>
                  Allow
                </button>
              </span>
            )}
            <label className="alert-toggle">
              <input
                type="checkbox"
                checked={draft.notifyWhenFocused}
                disabled={!draft.osNotifications}
                onChange={(event) => update({ notifyWhenFocused: event.target.checked })}
              />
              <span>Also while GpuTerm is in front (otherwise an in-app message)</span>
            </label>
          </div>

          {(problem || error) && (
            <div className="ssh-config-status error" role="alert">
              {problem ?? error}
            </div>
          )}
          <div className="button-row">
            <button
              type="button"
              className="primary-button"
              disabled={!dirty || saving || problem != null}
              onClick={() => void save()}
            >
              Save
            </button>
            <button
              type="button"
              className="secondary-button"
              disabled={!dirty || saving}
              onClick={() => {
                setDraft(saved);
                setError(null);
              }}
            >
              Discard
            </button>
            <button
              type="button"
              className="secondary-button"
              disabled={permission !== "granted"}
              onClick={test}
            >
              Send test
            </button>
          </div>

          <section className="alert-recent" aria-label="Recent alerts">
            <h4>Recent</h4>
            {recent.length === 0 ? (
              <div className="empty-list compact">No alerts since GpuTerm started</div>
            ) : (
              <ul>
                {recent.slice(0, 10).map((alert) => (
                  <li key={alert.id} className={alert.state}>
                    <time dateTime={new Date(alert.at * 1000).toISOString()}>
                      {new Date(alert.at * 1000).toLocaleTimeString()}
                    </time>
                    <span>
                      {alert.hostLabel && <strong>{alert.hostLabel}</strong>} {alert.title}
                      {alert.body && ` — ${alert.body}`}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </section>
          <small className="session-credential-hint">
            An alert notifies once when it starts and is shown here again when it clears.
          </small>
        </div>
      )}
    </ResourceDetailPopover>
  );
}
