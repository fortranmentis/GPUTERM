use crate::ssh::session::{open_ssh_session, SshTarget};
use serde::Serialize;
use ssh2::Session;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

const NVIDIA_SMI_QUERY: &str = "nvidia-smi --query-gpu=index,name,uuid,driver_version,power.draw,power.limit,temperature.gpu,utilization.gpu,utilization.memory,memory.total,memory.used,memory.free --format=csv,noheader,nounits";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuMetric {
    index: u32,
    name: String,
    uuid: String,
    driver_version: String,
    power_draw_w: Option<f64>,
    power_limit_w: Option<f64>,
    temperature_c: Option<f64>,
    gpu_util_percent: Option<f64>,
    mem_util_percent: Option<f64>,
    memory_total_mi_b: Option<u64>,
    memory_used_mi_b: Option<u64>,
    memory_free_mi_b: Option<u64>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GpuMetricsPayload {
    session_id: String,
    status: String,
    metrics: Vec<GpuMetric>,
    message: Option<String>,
    updated_at: u64,
}

pub fn start(app: AppHandle, target: SshTarget, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        let session = match open_ssh_session(&target) {
            Ok(session) => session,
            Err(error) => {
                emit_unavailable(
                    &app,
                    &target.session_id,
                    format!("GPU metrics unavailable: monitor SSH connection failed: {}", error),
                );
                return;
            }
        };

        while !stop.load(Ordering::SeqCst) {
            match collect_gpu_metrics(&session) {
                Ok(metrics) => emit_available(&app, &target.session_id, metrics),
                Err(error) => emit_unavailable(&app, &target.session_id, error),
            }

            for _ in 0..20 {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    });
}

fn collect_gpu_metrics(session: &Session) -> Result<Vec<GpuMetric>, String> {
    let mut channel = session
        .channel_session()
        .map_err(|error| format!("GPU metrics unavailable: failed to open command channel: {}", error))?;
    channel
        .exec(NVIDIA_SMI_QUERY)
        .map_err(|error| format!("GPU metrics unavailable: failed to execute nvidia-smi: {}", error))?;

    let mut stdout = String::new();
    channel
        .read_to_string(&mut stdout)
        .map_err(|error| format!("GPU metrics unavailable: failed to read nvidia-smi output: {}", error))?;

    let mut stderr = String::new();
    let _ = channel.stderr().read_to_string(&mut stderr);
    let _ = channel.wait_close();
    let exit_status = channel.exit_status().unwrap_or(-1);

    if exit_status != 0 {
        let detail = if stderr.trim().is_empty() {
            "nvidia-smi failed or no NVIDIA GPU is available".to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(format!("GPU metrics unavailable: {}", detail));
    }

    parse_nvidia_smi_csv(&stdout)
}

fn parse_nvidia_smi_csv(output: &str) -> Result<Vec<GpuMetric>, String> {
    let mut metrics = Vec::new();

    for line in output.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        if fields.len() != 12 {
            return Err(format!(
                "GPU metrics unavailable: unexpected nvidia-smi column count {}",
                fields.len()
            ));
        }

        metrics.push(GpuMetric {
            index: fields[0]
                .parse::<u32>()
                .map_err(|_| "GPU metrics unavailable: invalid GPU index".to_string())?,
            name: fields[1].to_string(),
            uuid: fields[2].to_string(),
            driver_version: fields[3].to_string(),
            power_draw_w: parse_optional_f64(fields[4]),
            power_limit_w: parse_optional_f64(fields[5]),
            temperature_c: parse_optional_f64(fields[6]),
            gpu_util_percent: parse_optional_f64(fields[7]),
            mem_util_percent: parse_optional_f64(fields[8]),
            memory_total_mi_b: parse_optional_u64(fields[9]),
            memory_used_mi_b: parse_optional_u64(fields[10]),
            memory_free_mi_b: parse_optional_u64(fields[11]),
        });
    }

    if metrics.is_empty() {
        return Err("GPU metrics unavailable: no GPU rows returned by nvidia-smi".to_string());
    }

    Ok(metrics)
}

fn parse_optional_f64(value: &str) -> Option<f64> {
    let normalized = value.trim();
    if normalized.is_empty()
        || normalized.eq_ignore_ascii_case("N/A")
        || normalized.eq_ignore_ascii_case("[N/A]")
        || normalized.eq_ignore_ascii_case("Not Supported")
    {
        return None;
    }
    normalized.parse::<f64>().ok()
}

fn parse_optional_u64(value: &str) -> Option<u64> {
    parse_optional_f64(value).map(|number| number.round() as u64)
}

fn emit_available(app: &AppHandle, session_id: &str, metrics: Vec<GpuMetric>) {
    let _ = app.emit(
        "gpu-metrics",
        GpuMetricsPayload {
            session_id: session_id.to_string(),
            status: "available".to_string(),
            metrics,
            message: None,
            updated_at: now_epoch_ms(),
        },
    );
}

fn emit_unavailable(app: &AppHandle, session_id: &str, message: String) {
    let _ = app.emit(
        "gpu-metrics",
        GpuMetricsPayload {
            session_id: session_id.to_string(),
            status: "unavailable".to_string(),
            metrics: Vec::new(),
            message: Some(message),
            updated_at: now_epoch_ms(),
        },
    );
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}
