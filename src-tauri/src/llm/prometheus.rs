//! Prometheus text exposition parser, written here rather than taken from a
//! crate.
//!
//! Every candidate was checked and rejected:
//!
//! * `prometheus-parse` matches metric names with `\w`, which excludes `:`.
//!   vLLM namespaces every metric as `vllm:…`, so it returns an empty scrape
//!   with no error — indistinguishable from a server exposing nothing.
//! * `openmetrics-parser` rejects colons and uppercase, and is LGPL-3.0, which
//!   does not suit a statically linked binary under PolyForm-Noncommercial.
//! * `nom-openmetrics` gets names right but is `all_consuming`, so one
//!   unrecognized line fails the entire scrape, and it has a stray `eprintln!`
//!   on the `+Inf` path.
//! * `prometheus-scraper` is a month old with negligible adoption.
//!
//! Design rules here: a malformed line is skipped, never fatal; label sets are
//! preserved exactly so per-model series stay distinct and are never summed
//! together.

use std::collections::BTreeMap;

pub type Labels = BTreeMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    Counter,
    Gauge,
    Histogram,
    Summary,
    Untyped,
}

/// One `name{labels} value` line.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub name: String,
    pub labels: Labels,
    pub value: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Scrape {
    samples: Vec<Sample>,
    types: BTreeMap<String, MetricKind>,
    /// Lines that could not be read. Non-fatal, but worth surfacing.
    pub malformed_lines: usize,
}

/// A histogram for one label set: cumulative buckets plus its sum and count.
#[derive(Debug, Clone, PartialEq)]
pub struct Histogram {
    pub labels: Labels,
    /// `(le, cumulative_count)`, ascending by `le`.
    pub buckets: Vec<(f64, f64)>,
    pub sum: Option<f64>,
    pub count: Option<f64>,
}

impl Scrape {
    /// Every sample with this exact metric name.
    pub fn samples(&self, name: &str) -> Vec<&Sample> {
        self.samples
            .iter()
            .filter(|sample| sample.name == name)
            .collect()
    }

    /// Sums a gauge or counter across all its label sets.
    ///
    /// Correct for `vllm:num_requests_running` and friends, where each engine
    /// or model reports its own series and the total is what the card shows.
    /// Never used for histograms, where summing across label sets is wrong.
    pub fn sum(&self, name: &str) -> Option<f64> {
        let matching = self.samples(name);
        if matching.is_empty() {
            return None;
        }
        let total: f64 = matching
            .iter()
            .map(|sample| sample.value)
            .filter(|value| value.is_finite())
            .sum();
        Some(total)
    }

    /// True when the server exposed the metric at all, regardless of value.
    /// Distinguishes "not supported by this version" from "zero".
    pub fn has(&self, name: &str) -> bool {
        self.samples.iter().any(|sample| sample.name == name)
            || self.types.contains_key(name)
    }

    /// The declared type, when the exporter emitted a `# TYPE` line.
    pub(crate) fn kind(&self, name: &str) -> Option<MetricKind> {
        self.types.get(name).copied()
    }

    /// Groups `_bucket`, `_sum`, and `_count` for a histogram base name into one
    /// entry per label set. Buckets are sorted by `le` because exporters are not
    /// required to emit them in order.
    pub fn histograms(&self, base_name: &str) -> Vec<Histogram> {
        // An exporter that declared this name as something else is taken at its
        // word: interpolating percentiles out of a coincidentally-named
        // `_bucket` series would be inventing a number. A missing `# TYPE` is
        // allowed by the format, so only a contradicting one disqualifies.
        if self
            .kind(base_name)
            .is_some_and(|kind| kind != MetricKind::Histogram)
        {
            return Vec::new();
        }
        let bucket_name = format!("{}_bucket", base_name);
        let sum_name = format!("{}_sum", base_name);
        let count_name = format!("{}_count", base_name);

        let mut grouped: BTreeMap<Vec<(String, String)>, Histogram> = BTreeMap::new();

        for sample in &self.samples {
            if sample.name != bucket_name {
                continue;
            }
            let Some(le) = sample.labels.get("le").and_then(|raw| parse_value(raw)) else {
                continue;
            };
            let mut labels = sample.labels.clone();
            labels.remove("le");
            let key: Vec<(String, String)> = labels.clone().into_iter().collect();
            let entry = grouped.entry(key).or_insert_with(|| Histogram {
                labels,
                buckets: Vec::new(),
                sum: None,
                count: None,
            });
            entry.buckets.push((le, sample.value));
        }

        for sample in &self.samples {
            let field = if sample.name == sum_name {
                0
            } else if sample.name == count_name {
                1
            } else {
                continue;
            };
            let key: Vec<(String, String)> = sample.labels.clone().into_iter().collect();
            let entry = grouped.entry(key).or_insert_with(|| Histogram {
                labels: sample.labels.clone(),
                buckets: Vec::new(),
                sum: None,
                count: None,
            });
            if field == 0 {
                entry.sum = Some(sample.value);
            } else {
                entry.count = Some(sample.value);
            }
        }

        let mut histograms: Vec<Histogram> = grouped.into_values().collect();
        for histogram in &mut histograms {
            histogram
                .buckets
                .sort_by(|left, right| left.0.partial_cmp(&right.0).unwrap_or(std::cmp::Ordering::Equal));
        }
        histograms
    }
}

impl Histogram {
    /// Standard Prometheus histogram quantile over cumulative buckets.
    ///
    /// Returns `None` when there is not enough data to interpolate, rather than
    /// inventing a percentile from one bucket.
    pub fn quantile(&self, quantile: f64) -> Option<f64> {
        if !(0.0..=1.0).contains(&quantile) || self.buckets.len() < 2 {
            return None;
        }
        // The +Inf bucket carries the observation count.
        let total = self
            .buckets
            .last()
            .map(|(_, count)| *count)
            .or(self.count)
            .filter(|total| *total > 0.0)?;

        let rank = quantile * total;
        let mut previous_le = 0.0_f64;
        let mut previous_count = 0.0_f64;

        for (le, cumulative) in &self.buckets {
            if *cumulative < rank {
                previous_le = *le;
                previous_count = *cumulative;
                continue;
            }
            if le.is_infinite() {
                // Everything above the last finite bound: the best honest answer
                // is that bound, not infinity.
                return if previous_le > 0.0 {
                    Some(previous_le)
                } else {
                    None
                };
            }
            let bucket_count = cumulative - previous_count;
            if bucket_count <= 0.0 {
                return Some(*le);
            }
            let position = (rank - previous_count) / bucket_count;
            return Some(previous_le + (le - previous_le) * position);
        }
        None
    }
}

/// Parses a scrape body. Never fails: unreadable lines are counted and skipped
/// so one bad metric cannot cost the caller every other metric.
pub fn parse(body: &str) -> Scrape {
    let mut scrape = Scrape::default();

    for raw_line in body.lines() {
        let line = raw_line.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }
        if let Some(comment) = line.strip_prefix('#') {
            parse_comment(comment.trim(), &mut scrape);
            continue;
        }
        match parse_sample(line) {
            Some(sample) => scrape.samples.push(sample),
            None => scrape.malformed_lines += 1,
        }
    }
    scrape
}

/// Only `TYPE` is meaningful here. Any other comment, including a bare `#`, is
/// ignored rather than treated as an error.
fn parse_comment(comment: &str, scrape: &mut Scrape) {
    let mut parts = comment.split_whitespace();
    if parts.next() != Some("TYPE") {
        return;
    }
    let (Some(name), Some(kind)) = (parts.next(), parts.next()) else {
        return;
    };
    let kind = match kind.to_ascii_lowercase().as_str() {
        "counter" => MetricKind::Counter,
        "gauge" => MetricKind::Gauge,
        "histogram" => MetricKind::Histogram,
        "summary" => MetricKind::Summary,
        _ => MetricKind::Untyped,
    };
    scrape.types.insert(name.to_string(), kind);
}

fn is_name_start(character: char) -> bool {
    character.is_ascii_alphabetic() || character == '_' || character == ':'
}

fn is_name_char(character: char) -> bool {
    is_name_start(character) || character.is_ascii_digit()
}

fn parse_sample(line: &str) -> Option<Sample> {
    let bytes: Vec<char> = line.chars().collect();
    let mut index = 0;

    if !bytes.first().copied().is_some_and(is_name_start) {
        return None;
    }
    while index < bytes.len() && is_name_char(bytes[index]) {
        index += 1;
    }
    let name: String = bytes[..index].iter().collect();

    let mut labels = Labels::new();
    if bytes.get(index) == Some(&'{') {
        index += 1;
        let (parsed, next) = parse_labels(&bytes, index)?;
        labels = parsed;
        index = next;
    }

    // One or more spaces or tabs; not exactly one.
    let mut saw_separator = false;
    while index < bytes.len() && (bytes[index] == ' ' || bytes[index] == '\t') {
        saw_separator = true;
        index += 1;
    }
    if !saw_separator {
        return None;
    }

    let rest: String = bytes[index..].iter().collect();
    // A trailing timestamp is allowed and ignored.
    let value_token = rest.split_whitespace().next()?;
    let value = parse_value(value_token)?;

    Some(Sample {
        name,
        labels,
        value,
    })
}

/// Scans a label set with proper quoted-string handling.
///
/// Splitting on `,` and `=` would corrupt any label value containing a comma or
/// an escaped quote, which a HuggingFace `model_name` can easily contain.
fn parse_labels(bytes: &[char], mut index: usize) -> Option<(Labels, usize)> {
    let mut labels = Labels::new();

    loop {
        while index < bytes.len() && bytes[index].is_whitespace() {
            index += 1;
        }
        if bytes.get(index) == Some(&'}') {
            return Some((labels, index + 1));
        }

        let start = index;
        while index < bytes.len() && is_name_char(bytes[index]) {
            index += 1;
        }
        if index == start {
            return None;
        }
        let key: String = bytes[start..index].iter().collect();

        while index < bytes.len() && bytes[index].is_whitespace() {
            index += 1;
        }
        if bytes.get(index) != Some(&'=') {
            return None;
        }
        index += 1;
        while index < bytes.len() && bytes[index].is_whitespace() {
            index += 1;
        }
        if bytes.get(index) != Some(&'"') {
            return None;
        }
        index += 1;

        let mut value = String::new();
        loop {
            match bytes.get(index) {
                None => return None,
                Some('\\') => {
                    index += 1;
                    match bytes.get(index) {
                        Some('n') => value.push('\n'),
                        Some('t') => value.push('\t'),
                        Some(escaped) => value.push(*escaped),
                        None => return None,
                    }
                    index += 1;
                }
                Some('"') => {
                    index += 1;
                    break;
                }
                Some(character) => {
                    value.push(*character);
                    index += 1;
                }
            }
        }
        labels.insert(key, value);

        while index < bytes.len() && bytes[index].is_whitespace() {
            index += 1;
        }
        match bytes.get(index) {
            // A trailing comma before `}` is permitted.
            Some(',') => index += 1,
            Some('}') => return Some((labels, index + 1)),
            _ => return None,
        }
    }
}

/// Parses a Prometheus float. Go writes `NaN`, `+Inf`, `-Inf`, and `Infinity`
/// in mixed case; Rust's `from_str` only accepts lowercase, so normalize first.
pub fn parse_value(token: &str) -> Option<f64> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "nan" | "+nan" | "-nan" => return Some(f64::NAN),
        "inf" | "+inf" | "infinity" | "+infinity" => return Some(f64::INFINITY),
        "-inf" | "-infinity" => return Some(f64::NEG_INFINITY),
        _ => {}
    }
    trimmed.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real vLLM scrape: colon-namespaced names, several label
    /// sets per metric, and a histogram.
    const VLLM_METRICS: &str = r#"
# HELP vllm:num_requests_running Number of requests currently running on GPU.
# TYPE vllm:num_requests_running gauge
vllm:num_requests_running{engine="0",model_name="meta-llama/Llama-3-8B"} 3.0
vllm:num_requests_running{engine="1",model_name="meta-llama/Llama-3-8B"} 2.0
# HELP vllm:num_requests_waiting Number of requests waiting to be processed.
# TYPE vllm:num_requests_waiting gauge
vllm:num_requests_waiting{engine="0",model_name="meta-llama/Llama-3-8B"} 7.0
# TYPE vllm:kv_cache_usage_perc gauge
vllm:kv_cache_usage_perc{engine="0",model_name="meta-llama/Llama-3-8B"} 0.42
# TYPE vllm:prompt_tokens_total counter
vllm:prompt_tokens_total{model_name="meta-llama/Llama-3-8B"} 1.7560473e+07
# TYPE vllm:generation_tokens_total counter
vllm:generation_tokens_total{model_name="meta-llama/Llama-3-8B"} 512345
# TYPE vllm:e2e_request_latency_seconds histogram
vllm:e2e_request_latency_seconds_bucket{le="0.5",model_name="m"} 10
vllm:e2e_request_latency_seconds_bucket{le="1.0",model_name="m"} 60
vllm:e2e_request_latency_seconds_bucket{le="2.0",model_name="m"} 90
vllm:e2e_request_latency_seconds_bucket{le="+Inf",model_name="m"} 100
vllm:e2e_request_latency_seconds_sum{model_name="m"} 91.5
vllm:e2e_request_latency_seconds_count{model_name="m"} 100
"#;

    #[test]
    fn colon_namespaced_names_are_parsed() {
        // The exact failure that disqualified prometheus-parse: `\w` excludes
        // `:`, so every vllm metric silently vanished.
        let scrape = parse(VLLM_METRICS);
        assert!(scrape.has("vllm:num_requests_running"));
        assert_eq!(scrape.kind("vllm:num_requests_running"), Some(MetricKind::Gauge));
        assert_eq!(scrape.samples("vllm:num_requests_running").len(), 2);
    }

    #[test]
    fn label_sets_stay_distinct_and_gauges_sum_across_them() {
        let scrape = parse(VLLM_METRICS);
        let samples = scrape.samples("vllm:num_requests_running");
        assert_eq!(samples[0].labels.get("engine").map(String::as_str), Some("0"));
        assert_eq!(samples[1].labels.get("engine").map(String::as_str), Some("1"));
        assert_eq!(scrape.sum("vllm:num_requests_running"), Some(5.0));
        assert_eq!(scrape.sum("vllm:num_requests_waiting"), Some(7.0));
    }

    #[test]
    fn scientific_notation_is_read() {
        let scrape = parse(VLLM_METRICS);
        assert_eq!(scrape.sum("vllm:prompt_tokens_total"), Some(17_560_473.0));
    }

    #[test]
    fn a_missing_metric_is_absent_rather_than_zero() {
        let scrape = parse(VLLM_METRICS);
        // Older and newer vLLM builds differ on which of these exist.
        assert!(!scrape.has("vllm:num_requests_swapped"));
        assert_eq!(scrape.sum("vllm:num_requests_swapped"), None);
        assert!(!scrape.has("vllm:gpu_cache_usage_perc"));
    }

    #[test]
    fn histogram_buckets_group_by_label_set_and_sort_by_le() {
        let scrape = parse(VLLM_METRICS);
        let histograms = scrape.histograms("vllm:e2e_request_latency_seconds");
        assert_eq!(histograms.len(), 1);
        let histogram = &histograms[0];
        assert_eq!(histogram.count, Some(100.0));
        assert_eq!(histogram.sum, Some(91.5));
        assert_eq!(histogram.buckets.len(), 4);
        assert_eq!(histogram.buckets[0].0, 0.5);
        assert!(histogram.buckets[3].0.is_infinite(), "+Inf sorts last");
        // `le` must not leak into the series identity.
        assert!(!histogram.labels.contains_key("le"));
    }

    #[test]
    fn buckets_are_sorted_even_when_the_exporter_emits_them_out_of_order() {
        let body = "\
# TYPE h histogram
h_bucket{le=\"+Inf\"} 100
h_bucket{le=\"1.0\"} 60
h_bucket{le=\"0.5\"} 10
h_bucket{le=\"2.0\"} 90
";
        let histogram = &parse(body).histograms("h")[0];
        let bounds: Vec<f64> = histogram.buckets.iter().map(|(le, _)| *le).collect();
        assert_eq!(bounds[0], 0.5);
        assert_eq!(bounds[1], 1.0);
        assert_eq!(bounds[2], 2.0);
        assert!(bounds[3].is_infinite());
    }

    #[test]
    fn quantiles_interpolate_over_cumulative_buckets() {
        let scrape = parse(VLLM_METRICS);
        let histogram = &scrape.histograms("vllm:e2e_request_latency_seconds")[0];

        // rank = 50 falls in (0.5, 1.0], which spans counts 10..60.
        // 0.5 + (1.0 - 0.5) * (50 - 10) / 50 = 0.9
        let p50 = histogram.quantile(0.5).unwrap();
        assert!((p50 - 0.9).abs() < 1e-9, "{p50}");

        // rank = 95 falls in (2.0, +Inf]; the honest answer is the last finite
        // bound rather than infinity.
        let p95 = histogram.quantile(0.95).unwrap();
        assert!((p95 - 2.0).abs() < 1e-9, "{p95}");
    }

    #[test]
    fn a_histogram_without_enough_data_yields_no_percentile() {
        let single = Histogram {
            labels: Labels::new(),
            buckets: vec![(f64::INFINITY, 5.0)],
            sum: None,
            count: Some(5.0),
        };
        assert_eq!(single.quantile(0.95), None, "one bucket cannot interpolate");

        let empty = Histogram {
            labels: Labels::new(),
            buckets: vec![(0.5, 0.0), (f64::INFINITY, 0.0)],
            sum: Some(0.0),
            count: Some(0.0),
        };
        assert_eq!(empty.quantile(0.5), None, "no observations yet");
    }

    #[test]
    fn nan_and_inf_values_parse_in_any_case() {
        assert!(parse_value("NaN").unwrap().is_nan());
        assert!(parse_value("nan").unwrap().is_nan());
        assert_eq!(parse_value("+Inf"), Some(f64::INFINITY));
        assert_eq!(parse_value("Inf"), Some(f64::INFINITY));
        assert_eq!(parse_value("-Inf"), Some(f64::NEG_INFINITY));
        assert_eq!(parse_value("Infinity"), Some(f64::INFINITY));
        assert_eq!(parse_value("3e-5"), Some(3e-5));
        assert_eq!(parse_value("not-a-number"), None);

        // A NaN sample must not poison a sum.
        let scrape = parse("# TYPE g gauge\ng{a=\"1\"} 5\ng{a=\"2\"} NaN\n");
        assert_eq!(scrape.sum("g"), Some(5.0));
    }

    #[test]
    fn label_values_with_commas_quotes_and_escapes_survive() {
        // Naive split(',')/split('=') corrupts exactly this.
        let body = r#"g{model_name="org/model,v2",note="say \"hi\"",path="a\\b"} 1"#;
        let scrape = parse(body);
        let sample = scrape.samples("g")[0];
        assert_eq!(
            sample.labels.get("model_name").map(String::as_str),
            Some("org/model,v2")
        );
        assert_eq!(sample.labels.get("note").map(String::as_str), Some(r#"say "hi""#));
        assert_eq!(sample.labels.get("path").map(String::as_str), Some(r"a\b"));
    }

    #[test]
    fn one_bad_line_does_not_lose_the_rest_of_the_scrape() {
        let body = "\
# TYPE good gauge
good 1
this line is nonsense {{{
good_two 2
g{unterminated=\"x 3
good_three 3
";
        let scrape = parse(body);
        assert_eq!(scrape.sum("good"), Some(1.0));
        assert_eq!(scrape.sum("good_two"), Some(2.0));
        assert_eq!(scrape.sum("good_three"), Some(3.0));
        assert!(scrape.malformed_lines >= 1);
    }

    #[test]
    fn tolerates_crlf_tabs_timestamps_and_stray_comments() {
        let body = "# a bare comment\r\n# TYPE g gauge\r\ng\t42 1700000000000\r\ng2 7";
        let scrape = parse(body);
        assert_eq!(scrape.sum("g"), Some(42.0), "tab separator and timestamp");
        assert_eq!(scrape.sum("g2"), Some(7.0), "no trailing newline");
        assert_eq!(scrape.malformed_lines, 0, "a bare # comment is not an error");
    }

    #[test]
    fn empty_and_trailing_comma_label_sets_parse() {
        let scrape = parse("a{} 1\nb{x=\"1\",} 2\n");
        assert_eq!(scrape.sum("a"), Some(1.0));
        assert_eq!(scrape.sum("b"), Some(2.0));
    }
}
