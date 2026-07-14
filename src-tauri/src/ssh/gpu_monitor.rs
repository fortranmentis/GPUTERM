use crate::ssh::parse_util::{parse_optional_f64, parse_optional_u64};
use serde::Serialize;

pub const NVIDIA_SMI_QUERY: &str = "nvidia-smi --query-gpu=index,name,uuid,driver_version,power.draw,power.limit,temperature.gpu,utilization.gpu,utilization.memory,memory.total,memory.used,memory.free --format=csv,noheader,nounits";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuMetric {
    pub index: u32,
    pub name: String,
    pub uuid: String,
    pub driver_version: String,
    pub power_draw_w: Option<f64>,
    pub power_limit_w: Option<f64>,
    pub temperature_c: Option<f64>,
    pub gpu_util_percent: Option<f64>,
    pub mem_util_percent: Option<f64>,
    pub memory_total_mi_b: Option<u64>,
    pub memory_used_mi_b: Option<u64>,
    pub memory_free_mi_b: Option<u64>,
}

pub fn parse_nvidia_smi_csv(output: &str) -> Result<Vec<GpuMetric>, String> {
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
