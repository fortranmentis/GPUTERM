//! Threshold alerts on telemetry (GPU temperature, VRAM, an idle GPU holding
//! memory, RAM, disks) and on LLM runtimes turning critical.
//!
//! Rules are evaluated here, on every telemetry reading, so they fire whether
//! or not a window is open. A reading must stay over its threshold for the
//! rule's `sustain_secs` before it fires, and clears only once it falls below
//! `threshold - hysteresis`, so a value hovering at the line does not flap.
//! A fired alert notifies once; it does not repeat while it stays firing, and
//! the same alert is not notified again within `cooldown_secs`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::llm::severity::SeverityLevel;
use crate::ssh::session::AppState;

/// Sent for every fired or resolved alert.
pub const ALERT_EVENT: &str = "telemetry-alert";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThresholdRule {
    pub enabled: bool,
    pub threshold: f64,
    /// Clears below `threshold - hysteresis`.
    pub hysteresis: f64,
    pub sustain_secs: u64,
}

impl Default for ThresholdRule {
    fn default() -> Self {
        Self::new(true, 90.0, 5.0, 60)
    }
}

impl ThresholdRule {
    const fn new(enabled: bool, threshold: f64, hysteresis: f64, sustain_secs: u64) -> Self {
        Self {
            enabled,
            threshold,
            hysteresis,
            sustain_secs,
        }
    }
}

/// A GPU that has gone quiet while still holding memory: usually a finished
/// or stuck job that never released its allocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IdleGpuRule {
    pub enabled: bool,
    pub util_below: f64,
    pub min_vram_percent: f64,
    pub sustain_secs: u64,
}

impl Default for IdleGpuRule {
    fn default() -> Self {
        Self {
            enabled: false,
            util_below: 5.0,
            min_vram_percent: 10.0,
            sustain_secs: 900,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AlertSettings {
    pub enabled: bool,
    pub os_notifications: bool,
    /// Also send OS notifications while GpuTerm is the focused window;
    /// otherwise an in-app toast is shown instead.
    pub notify_when_focused: bool,
    pub cooldown_secs: u64,
    pub gpu_temp: ThresholdRule,
    pub gpu_vram: ThresholdRule,
    pub gpu_idle: IdleGpuRule,
    pub memory: ThresholdRule,
    pub disk: ThresholdRule,
    pub llm_critical: bool,
}

impl Default for AlertSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            os_notifications: true,
            notify_when_focused: false,
            cooldown_secs: 600,
            gpu_temp: ThresholdRule::new(true, 85.0, 5.0, 30),
            gpu_vram: ThresholdRule::new(true, 95.0, 5.0, 30),
            gpu_idle: IdleGpuRule::default(),
            memory: ThresholdRule::new(true, 90.0, 5.0, 60),
            disk: ThresholdRule::new(true, 90.0, 2.0, 0),
            llm_critical: true,
        }
    }
}

fn clamp_finite(value: f64, min: f64, max: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

fn sanitize_rule(rule: ThresholdRule, min: f64, max: f64) -> ThresholdRule {
    let threshold = clamp_finite(rule.threshold, min, max, max);
    ThresholdRule {
        enabled: rule.enabled,
        threshold,
        hysteresis: clamp_finite(rule.hysteresis, 0.0, 20.0_f64.min(threshold - min), 0.0),
        sustain_secs: rule.sustain_secs.min(3_600),
    }
}

/// Keeps hand-edited or out-of-range values from producing rules that can
/// never fire or never clear.
pub fn sanitize(settings: AlertSettings) -> AlertSettings {
    AlertSettings {
        cooldown_secs: settings.cooldown_secs.clamp(60, 86_400),
        gpu_temp: sanitize_rule(settings.gpu_temp, 30.0, 120.0),
        gpu_vram: sanitize_rule(settings.gpu_vram, 1.0, 100.0),
        gpu_idle: IdleGpuRule {
            enabled: settings.gpu_idle.enabled,
            util_below: clamp_finite(settings.gpu_idle.util_below, 0.0, 100.0, 5.0),
            min_vram_percent: clamp_finite(settings.gpu_idle.min_vram_percent, 0.0, 100.0, 10.0),
            sustain_secs: settings.gpu_idle.sustain_secs.clamp(60, 86_400),
        },
        memory: sanitize_rule(settings.memory, 1.0, 100.0),
        disk: sanitize_rule(settings.disk, 1.0, 100.0),
        ..settings
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Normal,
    /// Over the threshold since `since`, not yet for long enough.
    Pending {
        since: u64,
    },
    Firing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Fired,
    Resolved,
}

/// Advances one rule. `breach` and `clear` come from the caller's comparison;
/// None means there was no reading, which leaves the phase as it is.
pub fn step(
    phase: Phase,
    breach: Option<bool>,
    clear: Option<bool>,
    sustain_secs: u64,
    now: u64,
) -> (Phase, Option<Transition>) {
    match phase {
        Phase::Normal | Phase::Pending { .. } => {
            let Some(breach) = breach else {
                return (phase, None);
            };
            if !breach {
                return (Phase::Normal, None);
            }
            let since = match phase {
                Phase::Pending { since } => since,
                _ => now,
            };
            if now.saturating_sub(since) >= sustain_secs {
                (Phase::Firing, Some(Transition::Fired))
            } else {
                (Phase::Pending { since }, None)
            }
        }
        Phase::Firing => match clear {
            Some(true) => (Phase::Normal, Some(Transition::Resolved)),
            _ => (Phase::Firing, None),
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AlertKind {
    GpuTemp,
    GpuVram,
    GpuIdle,
    Memory,
    Disk,
    LlmCritical,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AlertKey {
    /// A session id, or "llm".
    scope: String,
    kind: AlertKind,
    /// The GPU key, mount point or LLM instance id; empty for memory.
    subject: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AlertEvent {
    pub id: String,
    pub scope: String,
    pub host_label: String,
    pub kind: AlertKind,
    pub subject: String,
    /// "fired" or "resolved".
    pub state: &'static str,
    pub value: Option<f64>,
    pub threshold: Option<f64>,
    pub at: u64,
    pub title: String,
    pub body: String,
    /// Whether this one should reach the user now (an OS notification or a
    /// toast). False for resolutions and for re-fires within the cooldown.
    pub notify: bool,
    /// Set when an OS notification was shown; the UI then skips its toast.
    pub os_delivered: bool,
}

/// What the rules read from one telemetry reading.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AlertInput {
    pub gpus: Vec<GpuAlertInput>,
    pub memory_percent: Option<f64>,
    /// Mount point and percent used, ignored filesystem types left out.
    pub disks: Vec<(String, Option<f64>)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GpuAlertInput {
    pub key: String,
    pub index: u32,
    pub name: String,
    pub temperature_c: Option<f64>,
    pub util_percent: Option<f64>,
    pub vram_percent: Option<f64>,
}

#[derive(Default)]
pub struct AlertEngine {
    settings: Mutex<AlertSettings>,
    phases: Mutex<HashMap<AlertKey, Phase>>,
    last_notified: Mutex<HashMap<AlertKey, u64>>,
}

/// One rule application, before it becomes an event.
struct Check<'a> {
    kind: AlertKind,
    subject: &'a str,
    subject_label: &'a str,
    value: Option<f64>,
    threshold: f64,
    breach: Option<bool>,
    clear: Option<bool>,
    sustain_secs: u64,
}

fn threshold_check<'a>(
    kind: AlertKind,
    subject: &'a str,
    subject_label: &'a str,
    rule: &ThresholdRule,
    value: Option<f64>,
) -> Option<Check<'a>> {
    rule.enabled.then(|| Check {
        kind,
        subject,
        subject_label,
        value,
        threshold: rule.threshold,
        breach: value.map(|value| value >= rule.threshold),
        clear: value.map(|value| value < rule.threshold - rule.hysteresis),
        sustain_secs: rule.sustain_secs,
    })
}

fn describe(
    kind: AlertKind,
    label: &str,
    value: Option<f64>,
    threshold: f64,
    fired: bool,
) -> (String, String) {
    let value = value
        .map(|value| format!("{:.0}", value))
        .unwrap_or_else(|| "?".into());
    let threshold = format!("{:.0}", threshold);
    match (kind, fired) {
        (AlertKind::GpuTemp, true) => (
            format!("{label} is hot"),
            format!("{value}°C (alert at {threshold}°C)"),
        ),
        (AlertKind::GpuTemp, false) => (format!("{label} cooled down"), format!("{value}°C")),
        (AlertKind::GpuVram, true) => (
            format!("{label} VRAM nearly full"),
            format!("{value}% used (alert at {threshold}%)"),
        ),
        (AlertKind::GpuVram, false) => (format!("{label} VRAM freed"), format!("{value}% used")),
        (AlertKind::GpuIdle, true) => (
            format!("{label} is idle but holds memory"),
            format!("{value}% VRAM held with almost no GPU use"),
        ),
        (AlertKind::GpuIdle, false) => (format!("{label} is no longer idle"), String::new()),
        (AlertKind::Memory, true) => (
            "Memory nearly full".into(),
            format!("{value}% used (alert at {threshold}%)"),
        ),
        (AlertKind::Memory, false) => ("Memory back to normal".into(), format!("{value}% used")),
        (AlertKind::Disk, true) => (
            format!("Disk {label} nearly full"),
            format!("{value}% used (alert at {threshold}%)"),
        ),
        (AlertKind::Disk, false) => (
            format!("Disk {label} back to normal"),
            format!("{value}% used"),
        ),
        (AlertKind::LlmCritical, _) => (label.to_string(), String::new()),
    }
}

impl AlertEngine {
    pub fn settings(&self) -> AlertSettings {
        self.settings
            .lock()
            .map(|settings| settings.clone())
            .unwrap_or_default()
    }

    pub fn set_settings(&self, settings: AlertSettings) {
        if let Ok(mut slot) = self.settings.lock() {
            *slot = settings;
        }
        // Thresholds may have moved; start every rule fresh rather than
        // keep judging against the old line.
        if let Ok(mut phases) = self.phases.lock() {
            phases.clear();
        }
    }

    /// Applies one rule, returning its event if it fired or resolved.
    fn apply(
        &self,
        scope: &str,
        host_label: &str,
        check: Check,
        now: u64,
        cooldown: u64,
    ) -> Option<AlertEvent> {
        let key = AlertKey {
            scope: scope.to_string(),
            kind: check.kind,
            subject: check.subject.to_string(),
        };
        let transition = {
            let mut phases = self.phases.lock().ok()?;
            let phase = phases.get(&key).copied().unwrap_or(Phase::Normal);
            let (next, transition) =
                step(phase, check.breach, check.clear, check.sustain_secs, now);
            if next == Phase::Normal {
                phases.remove(&key);
            } else {
                phases.insert(key.clone(), next);
            }
            transition?
        };
        let fired = transition == Transition::Fired;
        let notify = fired && {
            let mut last = self.last_notified.lock().ok()?;
            let due = last
                .get(&key)
                .is_none_or(|at| now.saturating_sub(*at) >= cooldown);
            if due {
                last.insert(key.clone(), now);
            }
            due
        };
        let (title, body) = describe(
            check.kind,
            check.subject_label,
            check.value,
            check.threshold,
            fired,
        );
        Some(AlertEvent {
            id: uuid::Uuid::new_v4().to_string(),
            scope: scope.to_string(),
            host_label: host_label.to_string(),
            kind: check.kind,
            subject: check.subject.to_string(),
            state: if fired { "fired" } else { "resolved" },
            value: check.value,
            threshold: Some(check.threshold),
            at: now,
            title,
            body,
            notify,
            os_delivered: false,
        })
    }

    pub fn evaluate_telemetry(
        &self,
        scope: &str,
        host_label: &str,
        input: &AlertInput,
        now: u64,
    ) -> Vec<AlertEvent> {
        let settings = self.settings();
        if !settings.enabled {
            return Vec::new();
        }
        let mut checks = Vec::new();
        let gpu_labels: Vec<String> = input
            .gpus
            .iter()
            .map(|gpu| format!("GPU {} ({})", gpu.index, gpu.name))
            .collect();
        for (gpu, label) in input.gpus.iter().zip(&gpu_labels) {
            checks.extend(threshold_check(
                AlertKind::GpuTemp,
                &gpu.key,
                label,
                &settings.gpu_temp,
                gpu.temperature_c,
            ));
            checks.extend(threshold_check(
                AlertKind::GpuVram,
                &gpu.key,
                label,
                &settings.gpu_vram,
                gpu.vram_percent,
            ));
            let idle = &settings.gpu_idle;
            if idle.enabled {
                let idle_now = match (gpu.util_percent, gpu.vram_percent) {
                    (Some(util), Some(vram)) => {
                        Some(util < idle.util_below && vram >= idle.min_vram_percent)
                    }
                    _ => None,
                };
                checks.push(Check {
                    kind: AlertKind::GpuIdle,
                    subject: &gpu.key,
                    subject_label: label,
                    value: gpu.vram_percent,
                    threshold: idle.min_vram_percent,
                    breach: idle_now,
                    clear: idle_now.map(|idle| !idle),
                    sustain_secs: idle.sustain_secs,
                });
            }
        }
        checks.extend(threshold_check(
            AlertKind::Memory,
            "",
            "Memory",
            &settings.memory,
            input.memory_percent,
        ));
        for (mount, percent) in &input.disks {
            checks.extend(threshold_check(
                AlertKind::Disk,
                mount,
                mount,
                &settings.disk,
                *percent,
            ));
        }
        checks
            .into_iter()
            .filter_map(|check| self.apply(scope, host_label, check, now, settings.cooldown_secs))
            .collect()
    }

    /// An LLM instance's severity changed. Only a move into Critical from a
    /// known healthy-ish level fires: an instance that is simply down at
    /// startup (Unknown -> Critical) would otherwise notify on every launch.
    pub fn llm_transition(
        &self,
        instance_id: &str,
        name: &str,
        from: Option<SeverityLevel>,
        to: SeverityLevel,
        reasons: &[String],
        now: u64,
    ) -> Option<AlertEvent> {
        let settings = self.settings();
        if !settings.enabled || !settings.llm_critical {
            return None;
        }
        let from_known = matches!(
            from,
            Some(SeverityLevel::Normal | SeverityLevel::Warning | SeverityLevel::Congested)
        );
        let fired = from_known && to == SeverityLevel::Critical;
        let resolved = from == Some(SeverityLevel::Critical) && to != SeverityLevel::Critical;
        if !fired && !resolved {
            return None;
        }
        let reason_text = reasons
            .iter()
            .map(|reason| reason.replace('_', " "))
            .collect::<Vec<_>>()
            .join(", ");
        let mut event = self.apply(
            "llm",
            name,
            Check {
                kind: AlertKind::LlmCritical,
                subject: instance_id,
                subject_label: name,
                value: None,
                threshold: 0.0,
                breach: Some(fired),
                clear: Some(resolved),
                sustain_secs: 0,
            },
            now,
            settings.cooldown_secs,
        )?;
        event.threshold = None;
        if fired {
            event.title = format!("{name} is critical");
            event.body = reason_text;
        } else {
            event.title = format!("{name} recovered");
        }
        Some(event)
    }

    /// Drops the state of a session that ended.
    pub fn forget_scope(&self, scope: &str) {
        if let Ok(mut phases) = self.phases.lock() {
            phases.retain(|key, _| key.scope != scope);
        }
    }
}

/// Shows an OS notification for each event that should notify (unless the
/// app is focused and the user asked for in-app toasts then), and sends every
/// event to the windows. Runs off the caller's thread: platform notification
/// APIs can block, and the caller is a telemetry or poller loop.
pub fn deliver(app: &AppHandle, events: Vec<AlertEvent>) {
    if events.is_empty() {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let settings = app
            .try_state::<AppState>()
            .map(|state| state.alerts.settings())
            .unwrap_or_default();
        let focused = app
            .get_webview_window("main")
            .and_then(|window| window.is_focused().ok())
            .unwrap_or(false);
        for mut event in events {
            if event.notify
                && settings.os_notifications
                && (settings.notify_when_focused || !focused)
            {
                use tauri_plugin_notification::NotificationExt;
                let title = if event.host_label.is_empty() {
                    event.title.clone()
                } else {
                    format!("{}: {}", event.host_label, event.title)
                };
                event.os_delivered = app
                    .notification()
                    .builder()
                    .title(title)
                    .body(event.body.clone())
                    .show()
                    .is_ok();
            }
            let _ = app.emit(ALERT_EVENT, &event);
        }
    });
}

#[tauri::command]
pub fn get_alert_settings(state: State<'_, AppState>) -> AlertSettings {
    state.alerts.settings()
}

#[tauri::command(async)]
pub fn update_alert_settings(
    state: State<'_, AppState>,
    settings: AlertSettings,
) -> Result<AlertSettings, String> {
    let settings = sanitize(settings);
    // Saved first, so the engine never runs rules the UI rolled back.
    crate::ssh::local_fs::modify_app_settings(|stored| stored.alerts = Some(settings.clone()))?;
    state.alerts.set_settings(settings.clone());
    Ok(settings)
}

/// The settings saved by a previous run, or the defaults.
pub fn load_persisted_settings() -> AlertSettings {
    match crate::ssh::local_fs::read_app_settings() {
        Ok(settings) => settings.alerts.map(sanitize).unwrap_or_default(),
        Err(error) => {
            eprintln!("GpuTerm: could not load alert settings: {error}");
            AlertSettings::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_800_000_000;

    #[test]
    fn a_breach_must_last_the_sustain_time_before_firing() {
        let (phase, transition) = step(Phase::Normal, Some(true), Some(false), 30, T0);
        assert_eq!((phase, transition), (Phase::Pending { since: T0 }, None));
        let (phase, transition) = step(phase, Some(true), Some(false), 30, T0 + 29);
        assert_eq!(transition, None);
        let (phase, transition) = step(phase, Some(true), Some(false), 30, T0 + 30);
        assert_eq!(
            (phase, transition),
            (Phase::Firing, Some(Transition::Fired))
        );
    }

    #[test]
    fn a_short_breach_does_not_fire() {
        let (phase, _) = step(Phase::Normal, Some(true), Some(false), 30, T0);
        let (phase, transition) = step(phase, Some(false), Some(false), 30, T0 + 10);
        assert_eq!((phase, transition), (Phase::Normal, None));
    }

    #[test]
    fn a_missing_reading_holds_the_phase() {
        let pending = Phase::Pending { since: T0 };
        assert_eq!(step(pending, None, None, 30, T0 + 100), (pending, None));
        assert_eq!(
            step(Phase::Firing, None, None, 30, T0),
            (Phase::Firing, None)
        );
    }

    #[test]
    fn firing_clears_only_below_the_hysteresis_band() {
        let engine = AlertEngine::default();
        engine.set_settings(AlertSettings {
            memory: ThresholdRule::new(true, 90.0, 5.0, 0),
            ..AlertSettings::default()
        });
        let input = |memory: f64| AlertInput {
            memory_percent: Some(memory),
            ..AlertInput::default()
        };
        let fired = engine.evaluate_telemetry("s", "lab", &input(91.0), T0);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].state, "fired");
        assert!(fired[0].notify);
        assert_eq!(fired[0].title, "Memory nearly full");
        // Between 85 and 90: still firing, nothing new.
        assert!(engine
            .evaluate_telemetry("s", "lab", &input(87.0), T0 + 5)
            .is_empty());
        let resolved = engine.evaluate_telemetry("s", "lab", &input(84.0), T0 + 10);
        assert_eq!(resolved[0].state, "resolved");
        assert!(!resolved[0].notify);
    }

    #[test]
    fn a_quick_re_fire_is_not_notified_again_within_the_cooldown() {
        let engine = AlertEngine::default();
        engine.set_settings(AlertSettings {
            disk: ThresholdRule::new(true, 90.0, 2.0, 0),
            cooldown_secs: 600,
            ..AlertSettings::default()
        });
        let input = |percent: f64| AlertInput {
            disks: vec![("/data".to_string(), Some(percent))],
            ..AlertInput::default()
        };
        assert!(engine.evaluate_telemetry("s", "lab", &input(95.0), T0)[0].notify);
        engine.evaluate_telemetry("s", "lab", &input(50.0), T0 + 60);
        let again = engine.evaluate_telemetry("s", "lab", &input(95.0), T0 + 120);
        assert_eq!(again[0].state, "fired");
        assert!(!again[0].notify);
        engine.evaluate_telemetry("s", "lab", &input(50.0), T0 + 180);
        assert!(engine.evaluate_telemetry("s", "lab", &input(95.0), T0 + 700)[0].notify);
    }

    #[test]
    fn each_gpu_and_mount_is_tracked_separately() {
        let engine = AlertEngine::default();
        engine.set_settings(AlertSettings {
            gpu_temp: ThresholdRule::new(true, 85.0, 5.0, 0),
            disk: ThresholdRule::new(true, 90.0, 2.0, 0),
            ..AlertSettings::default()
        });
        let input = AlertInput {
            gpus: vec![
                GpuAlertInput {
                    key: "GPU-a".into(),
                    index: 0,
                    name: "A100".into(),
                    temperature_c: Some(88.0),
                    ..GpuAlertInput::default()
                },
                GpuAlertInput {
                    key: "GPU-b".into(),
                    index: 1,
                    name: "A100".into(),
                    temperature_c: Some(60.0),
                    ..GpuAlertInput::default()
                },
            ],
            disks: vec![("/".into(), Some(40.0)), ("/data".into(), Some(97.0))],
            ..AlertInput::default()
        };
        let events = engine.evaluate_telemetry("s", "lab", &input, T0);
        let mut subjects: Vec<&str> = events.iter().map(|event| event.subject.as_str()).collect();
        subjects.sort();
        assert_eq!(subjects, ["/data", "GPU-a"]);
        assert_eq!(
            events.iter().find(|e| e.subject == "GPU-a").unwrap().title,
            "GPU 0 (A100) is hot"
        );
    }

    #[test]
    fn an_idle_gpu_holding_memory_fires_when_enabled() {
        let engine = AlertEngine::default();
        engine.set_settings(AlertSettings {
            gpu_idle: IdleGpuRule {
                enabled: true,
                util_below: 5.0,
                min_vram_percent: 10.0,
                sustain_secs: 60,
            },
            ..AlertSettings::default()
        });
        let input = |util: f64, vram: f64| AlertInput {
            gpus: vec![GpuAlertInput {
                key: "GPU-a".into(),
                name: "A100".into(),
                util_percent: Some(util),
                vram_percent: Some(vram),
                ..GpuAlertInput::default()
            }],
            ..AlertInput::default()
        };
        assert!(engine
            .evaluate_telemetry("s", "lab", &input(0.0, 40.0), T0)
            .is_empty());
        let events = engine.evaluate_telemetry("s", "lab", &input(1.0, 40.0), T0 + 60);
        assert_eq!(events[0].kind, AlertKind::GpuIdle);
        // Busy again: resolved.
        let events = engine.evaluate_telemetry("s", "lab", &input(80.0, 40.0), T0 + 65);
        assert_eq!(events[0].state, "resolved");
    }

    #[test]
    fn disabled_alerts_produce_nothing() {
        let engine = AlertEngine::default();
        engine.set_settings(AlertSettings {
            enabled: false,
            ..AlertSettings::default()
        });
        let input = AlertInput {
            memory_percent: Some(100.0),
            ..AlertInput::default()
        };
        assert!(engine.evaluate_telemetry("s", "lab", &input, T0).is_empty());
    }

    #[test]
    fn llm_alerts_fire_only_on_a_move_into_critical_from_a_known_level() {
        let engine = AlertEngine::default();
        let reasons = vec!["kv_cache_full".to_string()];
        assert!(engine
            .llm_transition(
                "i",
                "vLLM",
                Some(SeverityLevel::Unknown),
                SeverityLevel::Critical,
                &reasons,
                T0
            )
            .is_none());
        assert!(engine
            .llm_transition("i", "vLLM", None, SeverityLevel::Critical, &reasons, T0)
            .is_none());
        let fired = engine
            .llm_transition(
                "i",
                "vLLM",
                Some(SeverityLevel::Warning),
                SeverityLevel::Critical,
                &reasons,
                T0,
            )
            .unwrap();
        assert_eq!(fired.state, "fired");
        assert_eq!(fired.title, "vLLM is critical");
        assert_eq!(fired.body, "kv cache full");
        let resolved = engine
            .llm_transition(
                "i",
                "vLLM",
                Some(SeverityLevel::Critical),
                SeverityLevel::Normal,
                &[],
                T0 + 5,
            )
            .unwrap();
        assert_eq!(resolved.state, "resolved");
    }

    #[test]
    fn forgetting_a_session_resets_its_rules() {
        let engine = AlertEngine::default();
        engine.set_settings(AlertSettings {
            memory: ThresholdRule::new(true, 90.0, 5.0, 0),
            ..AlertSettings::default()
        });
        let input = AlertInput {
            memory_percent: Some(95.0),
            ..AlertInput::default()
        };
        engine.evaluate_telemetry("s", "lab", &input, T0);
        engine.forget_scope("s");
        // Fires again rather than staying silently "already firing".
        let events = engine.evaluate_telemetry("s", "lab", &input, T0 + 1_000);
        assert_eq!(events[0].state, "fired");
    }

    #[test]
    fn sanitize_keeps_rules_usable() {
        let settings = sanitize(AlertSettings {
            cooldown_secs: 1,
            gpu_temp: ThresholdRule::new(true, 500.0, 50.0, 99_999),
            memory: ThresholdRule::new(true, f64::NAN, -3.0, 10),
            ..AlertSettings::default()
        });
        assert_eq!(settings.cooldown_secs, 60);
        assert_eq!(settings.gpu_temp.threshold, 120.0);
        assert_eq!(settings.gpu_temp.hysteresis, 20.0);
        assert_eq!(settings.gpu_temp.sustain_secs, 3_600);
        assert_eq!(settings.memory.threshold, 100.0);
        assert_eq!(settings.memory.hysteresis, 0.0);
    }

    #[test]
    fn saved_settings_fill_missing_fields_with_defaults() {
        let partial: AlertSettings =
            serde_json::from_str(r#"{"enabled":false,"gpuTemp":{"threshold":80}}"#).unwrap();
        assert!(!partial.enabled);
        assert_eq!(partial.gpu_temp.threshold, 80.0);
        // Missing fields in a rule fall back to the generic rule default.
        assert!(partial.gpu_temp.enabled);
        assert_eq!(partial.memory, AlertSettings::default().memory);
    }
}
