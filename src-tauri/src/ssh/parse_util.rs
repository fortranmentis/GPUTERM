//! Shared parsers for remote command output used by telemetry and detail collection.

use std::collections::HashMap;

/// Splits output annotated with `__SECTION__` marker lines into named sections.
pub(crate) fn split_sections(output: &str) -> HashMap<String, String> {
    let mut sections = HashMap::new();
    let mut current = None::<String>;
    for line in output.lines() {
        if let Some(section) = line
            .trim()
            .strip_prefix("__")
            .and_then(|value| value.strip_suffix("__"))
        {
            current = Some(section.to_string());
            sections.entry(section.to_string()).or_insert_with(String::new);
        } else if let Some(section) = &current {
            let value = sections.entry(section.clone()).or_insert_with(String::new);
            value.push_str(line);
            value.push('\n');
        }
    }
    sections
}

pub(crate) fn required_section<'a>(
    sections: &'a HashMap<String, String>,
    name: &str,
) -> Result<&'a str, String> {
    sections
        .get(name)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("missing {} section", name))
}

pub(crate) fn parse_loadavg(output: &str) -> (Option<f64>, Option<f64>, Option<f64>) {
    let mut values = output.split_whitespace();
    (
        values.next().and_then(|value| value.parse().ok()),
        values.next().and_then(|value| value.parse().ok()),
        values.next().and_then(|value| value.parse().ok()),
    )
}

pub(crate) fn parse_cpu_model(cpuinfo: &str) -> Option<String> {
    cpuinfo.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == "model name").then(|| value.trim().to_string())
    })
}

/// Average of all `cpu MHz` rows in /proc/cpuinfo, converted to GHz.
pub(crate) fn parse_average_clock(cpuinfo: &str) -> Option<f64> {
    let values = cpuinfo
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim() == "cpu MHz").then(|| value.trim().parse::<f64>().ok()).flatten()
        })
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64 / 1000.0)
}

pub(crate) fn parse_lscpu_value(output: &str, key: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (line_key, value) = line.split_once(':')?;
        (line_key.trim() == key).then(|| value.trim().to_string())
    })
}

pub(crate) fn parse_first_u64(output: &str) -> Option<u64> {
    output.split_whitespace().next()?.parse().ok()
}

/// Parses /proc/meminfo style `Key: value kB` rows into a key → KiB map.
pub(crate) fn parse_meminfo_values(output: &str) -> HashMap<String, u64> {
    output
        .lines()
        .filter_map(|line| {
            let (key, rest) = line.split_once(':')?;
            let value = rest.split_whitespace().next()?.parse::<u64>().ok()?;
            Some((key.to_string(), value))
        })
        .collect()
}

pub(crate) fn kib_to_mib(value: u64) -> u64 {
    value / 1024
}

/// Trims and maps nvidia-smi / ps placeholder values ("N/A", "-", …) to None.
pub(crate) fn optional_string(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.eq_ignore_ascii_case("N/A")
        || value.eq_ignore_ascii_case("[N/A]")
        || value.eq_ignore_ascii_case("Not Supported")
        || value == "-"
    {
        None
    } else {
        Some(value.to_string())
    }
}

pub(crate) fn parse_optional_f64(value: &str) -> Option<f64> {
    optional_string(value)?.parse().ok()
}

pub(crate) fn parse_optional_u64(value: &str) -> Option<u64> {
    parse_optional_f64(value).map(|value| value.round() as u64)
}
