//! The last hour of CPU, memory, disk and GPU readings per session, for the
//! usage charts.
//!
//! Kept here rather than in the webview so the popovers and every detached
//! window draw the same series, and history keeps building while no chart is
//! open. Readings are averaged into fixed buckets, so memory use does not
//! depend on the telemetry interval. Nothing is written to disk.

use serde::Serialize;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Mutex;
use tauri::State;

use super::session::AppState;

pub const HISTORY_WINDOW_SECS: u64 = 3_600;
pub const HISTORY_BUCKET_SECS: u64 = 5;
pub const HISTORY_MAX_POINTS: usize = (HISTORY_WINDOW_SECS / HISTORY_BUCKET_SECS) as usize;

/// One telemetry reading, reduced to what the charts plot.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HistorySample {
    pub cpu_percent: Option<f64>,
    pub memory_percent: Option<f64>,
    pub swap_percent: Option<f64>,
    /// The fullest disk that is not an ignored filesystem type.
    pub disk_max_percent: Option<f64>,
    pub gpus: Vec<GpuSample>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GpuSample {
    /// See `gpu_history_key`.
    pub key: String,
    pub index: u32,
    pub name: String,
    pub util_percent: Option<f64>,
    pub vram_used_mi_b: Option<u64>,
    pub vram_total_mi_b: Option<u64>,
    pub temperature_c: Option<f64>,
    pub power_w: Option<f64>,
    pub power_limit_w: Option<f64>,
}

/// Identifies a GPU across readings: its UUID, or its index when the tool
/// reports none. The frontend builds the same key (`gpuHistoryKey`).
pub fn gpu_history_key(uuid: &str, index: u32) -> String {
    if uuid.trim().is_empty() {
        format!("idx:{}", index)
    } else {
        uuid.to_string()
    }
}

/// Running sum and count of the readings in one bucket.
#[derive(Debug, Clone, Copy, Default)]
struct Acc {
    sum: f64,
    count: u32,
}

impl Acc {
    fn add(&mut self, value: Option<f64>) {
        if let Some(value) = value.filter(|value| value.is_finite()) {
            self.sum += value;
            self.count += 1;
        }
    }

    /// None when nothing was read in the bucket, never a made-up zero.
    fn average(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum / f64::from(self.count))
    }
}

#[derive(Debug, Clone, Default)]
struct SystemBucket {
    at: u64,
    cpu: Acc,
    memory: Acc,
    swap: Acc,
    disk: Acc,
}

#[derive(Debug, Clone, Default)]
struct GpuBucket {
    at: u64,
    util: Acc,
    vram_percent: Acc,
    vram_used: Acc,
    temperature: Acc,
    power: Acc,
}

#[derive(Debug, Default)]
struct GpuSeries {
    index: u32,
    name: String,
    memory_total_mi_b: Option<u64>,
    power_limit_w: Option<f64>,
    buckets: VecDeque<GpuBucket>,
}

#[derive(Debug, Default)]
struct SessionHistory {
    system: VecDeque<SystemBucket>,
    gpus: BTreeMap<String, GpuSeries>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SystemHistoryPoint {
    pub captured_at: u64,
    pub cpu_percent: Option<f64>,
    pub memory_percent: Option<f64>,
    pub swap_percent: Option<f64>,
    pub disk_max_percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GpuHistoryPoint {
    pub captured_at: u64,
    pub util_percent: Option<f64>,
    pub vram_percent: Option<f64>,
    pub vram_used_mi_b: Option<f64>,
    pub temperature_c: Option<f64>,
    pub power_w: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GpuHistorySeries {
    pub key: String,
    pub index: u32,
    pub name: String,
    pub memory_total_mi_b: Option<u64>,
    pub power_limit_w: Option<f64>,
    pub points: Vec<GpuHistoryPoint>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryHistoryPayload {
    pub session_id: String,
    pub bucket_secs: u64,
    pub window_secs: u64,
    pub system: Vec<SystemHistoryPoint>,
    pub gpus: Vec<GpuHistorySeries>,
}

#[derive(Default)]
pub struct TelemetryHistory {
    sessions: Mutex<HashMap<String, SessionHistory>>,
}

fn bucket_start(now: u64) -> u64 {
    now - now % HISTORY_BUCKET_SECS
}

/// The bucket `now` falls in, added at the back if it is not there yet.
fn current_bucket<T: Default>(
    buckets: &mut VecDeque<T>,
    now: u64,
    at: impl Fn(&T) -> u64,
    with_at: impl FnOnce(u64) -> T,
) -> &mut T {
    let start = bucket_start(now);
    if buckets.back().map(&at) != Some(start) {
        buckets.push_back(with_at(start));
    }
    buckets.back_mut().expect("a bucket was just ensured")
}

fn trim<T>(buckets: &mut VecDeque<T>, now: u64, at: impl Fn(&T) -> u64) {
    let cutoff = now.saturating_sub(HISTORY_WINDOW_SECS);
    while buckets.front().is_some_and(|bucket| at(bucket) < cutoff) {
        buckets.pop_front();
    }
    while buckets.len() > HISTORY_MAX_POINTS {
        buckets.pop_front();
    }
}

fn percent(used: Option<u64>, total: Option<u64>) -> Option<f64> {
    match (used, total) {
        (Some(used), Some(total)) if total > 0 => Some(used as f64 / total as f64 * 100.0),
        _ => None,
    }
}

impl TelemetryHistory {
    pub fn record(&self, session_id: &str, now: u64, sample: HistorySample) {
        let Ok(mut sessions) = self.sessions.lock() else {
            return;
        };
        let history = sessions.entry(session_id.to_string()).or_default();

        let bucket = current_bucket(
            &mut history.system,
            now,
            |b| b.at,
            |at| SystemBucket {
                at,
                ..SystemBucket::default()
            },
        );
        bucket.cpu.add(sample.cpu_percent);
        bucket.memory.add(sample.memory_percent);
        bucket.swap.add(sample.swap_percent);
        bucket.disk.add(sample.disk_max_percent);
        trim(&mut history.system, now, |b| b.at);

        for gpu in sample.gpus {
            let series = history.gpus.entry(gpu.key.clone()).or_default();
            series.index = gpu.index;
            series.name = gpu.name;
            series.memory_total_mi_b = gpu.vram_total_mi_b.or(series.memory_total_mi_b);
            series.power_limit_w = gpu.power_limit_w.or(series.power_limit_w);
            let bucket = current_bucket(
                &mut series.buckets,
                now,
                |b| b.at,
                |at| GpuBucket {
                    at,
                    ..GpuBucket::default()
                },
            );
            bucket.util.add(gpu.util_percent);
            bucket
                .vram_percent
                .add(percent(gpu.vram_used_mi_b, gpu.vram_total_mi_b));
            bucket
                .vram_used
                .add(gpu.vram_used_mi_b.map(|used| used as f64));
            bucket.temperature.add(gpu.temperature_c);
            bucket.power.add(gpu.power_w);
        }
        // A GPU that stopped reporting ages out like any old reading.
        for series in history.gpus.values_mut() {
            trim(&mut series.buckets, now, |b| b.at);
        }
        history.gpus.retain(|_, series| !series.buckets.is_empty());
    }

    /// The session's history, limited to buckets starting at or after
    /// `since` so a poller only fetches what is new (the newest bucket is
    /// always resent, as it may have gained readings).
    pub fn snapshot(
        &self,
        session_id: &str,
        since: Option<u64>,
    ) -> Option<TelemetryHistoryPayload> {
        let sessions = self.sessions.lock().ok()?;
        let history = sessions.get(session_id)?;
        let since = since.unwrap_or(0);
        let system = history
            .system
            .iter()
            .filter(|bucket| bucket.at >= since)
            .map(|bucket| SystemHistoryPoint {
                captured_at: bucket.at,
                cpu_percent: bucket.cpu.average(),
                memory_percent: bucket.memory.average(),
                swap_percent: bucket.swap.average(),
                disk_max_percent: bucket.disk.average(),
            })
            .collect();
        let gpus = history
            .gpus
            .iter()
            .map(|(key, series)| GpuHistorySeries {
                key: key.clone(),
                index: series.index,
                name: series.name.clone(),
                memory_total_mi_b: series.memory_total_mi_b,
                power_limit_w: series.power_limit_w,
                points: series
                    .buckets
                    .iter()
                    .filter(|bucket| bucket.at >= since)
                    .map(|bucket| GpuHistoryPoint {
                        captured_at: bucket.at,
                        util_percent: bucket.util.average(),
                        vram_percent: bucket.vram_percent.average(),
                        vram_used_mi_b: bucket.vram_used.average(),
                        temperature_c: bucket.temperature.average(),
                        power_w: bucket.power.average(),
                    })
                    .collect(),
            })
            .collect();
        Some(TelemetryHistoryPayload {
            session_id: session_id.to_string(),
            bucket_secs: HISTORY_BUCKET_SECS,
            window_secs: HISTORY_WINDOW_SECS,
            system,
            gpus,
        })
    }

    pub fn remove(&self, session_id: &str) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.remove(session_id);
        }
    }
}

/// The session's usage history; `since` (epoch seconds) limits it to newer
/// buckets. None until the session's first reading.
#[tauri::command]
pub fn get_telemetry_history(
    state: State<'_, AppState>,
    session_id: String,
    since: Option<u64>,
) -> Option<TelemetryHistoryPayload> {
    state.telemetry_history.snapshot(&session_id, since)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_800_000_000;

    fn system(cpu: Option<f64>) -> HistorySample {
        HistorySample {
            cpu_percent: cpu,
            memory_percent: Some(50.0),
            ..HistorySample::default()
        }
    }

    fn gpu(key: &str, util: Option<f64>) -> GpuSample {
        GpuSample {
            key: key.to_string(),
            name: "GPU".to_string(),
            util_percent: util,
            vram_used_mi_b: Some(4_096),
            vram_total_mi_b: Some(8_192),
            ..GpuSample::default()
        }
    }

    #[test]
    fn readings_in_one_bucket_are_averaged() {
        let history = TelemetryHistory::default();
        let start = bucket_start(T0);
        history.record("s", start, system(Some(10.0)));
        history.record("s", start + 2, system(Some(30.0)));
        history.record("s", start + 4, system(None));
        let snapshot = history.snapshot("s", None).unwrap();
        assert_eq!(snapshot.system.len(), 1);
        // A missing reading is left out of the average, not counted as zero.
        assert_eq!(snapshot.system[0].cpu_percent, Some(20.0));
        assert_eq!(snapshot.system[0].captured_at, start);

        history.record("s", start + HISTORY_BUCKET_SECS, system(None));
        let snapshot = history.snapshot("s", None).unwrap();
        assert_eq!(snapshot.system.len(), 2);
        assert_eq!(snapshot.system[1].cpu_percent, None);
    }

    #[test]
    fn history_keeps_one_hour() {
        let history = TelemetryHistory::default();
        let start = bucket_start(T0);
        history.record("s", start, system(Some(1.0)));
        history.record("s", start + HISTORY_WINDOW_SECS, system(Some(2.0)));
        let snapshot = history.snapshot("s", None).unwrap();
        // The first bucket is exactly one window old: still kept.
        assert_eq!(snapshot.system.len(), 2);
        history.record(
            "s",
            start + HISTORY_WINDOW_SECS + HISTORY_BUCKET_SECS,
            system(Some(3.0)),
        );
        let snapshot = history.snapshot("s", None).unwrap();
        assert_eq!(snapshot.system[0].cpu_percent, Some(2.0));
        assert!(snapshot.system.len() <= HISTORY_MAX_POINTS);
    }

    #[test]
    fn since_limits_the_snapshot_to_newer_buckets() {
        let history = TelemetryHistory::default();
        let start = bucket_start(T0);
        for step in 0..4 {
            history.record(
                "s",
                start + step * HISTORY_BUCKET_SECS,
                system(Some(step as f64)),
            );
        }
        let snapshot = history
            .snapshot("s", Some(start + 2 * HISTORY_BUCKET_SECS))
            .unwrap();
        assert_eq!(snapshot.system.len(), 2);
        assert_eq!(snapshot.system[0].cpu_percent, Some(2.0));
        assert!(history.snapshot("other", None).is_none());
    }

    #[test]
    fn gpus_are_kept_apart_and_vram_becomes_a_percentage() {
        let history = TelemetryHistory::default();
        let mut sample = system(Some(5.0));
        sample.gpus = vec![gpu("GPU-a", Some(90.0)), gpu("GPU-b", Some(10.0))];
        history.record("s", T0, sample);
        let snapshot = history.snapshot("s", None).unwrap();
        assert_eq!(snapshot.gpus.len(), 2);
        let a = snapshot
            .gpus
            .iter()
            .find(|series| series.key == "GPU-a")
            .unwrap();
        assert_eq!(a.points[0].util_percent, Some(90.0));
        assert_eq!(a.points[0].vram_percent, Some(50.0));
        assert_eq!(a.memory_total_mi_b, Some(8_192));
    }

    #[test]
    fn a_gpu_that_stops_reporting_ages_out() {
        let history = TelemetryHistory::default();
        let mut sample = system(Some(5.0));
        sample.gpus = vec![gpu("GPU-a", Some(90.0))];
        history.record("s", T0, sample);
        history.record("s", T0 + HISTORY_WINDOW_SECS + 60, system(Some(5.0)));
        assert!(history.snapshot("s", None).unwrap().gpus.is_empty());
    }

    #[test]
    fn gpu_keys_fall_back_to_the_index() {
        assert_eq!(gpu_history_key("GPU-1234", 0), "GPU-1234");
        assert_eq!(gpu_history_key("", 2), "idx:2");
    }

    #[test]
    fn removing_a_session_drops_its_history() {
        let history = TelemetryHistory::default();
        history.record("s", T0, system(Some(1.0)));
        history.remove("s");
        assert!(history.snapshot("s", None).is_none());
    }
}
