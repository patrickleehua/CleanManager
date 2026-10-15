//! System status snapshot model (parity STS-01..03).
//!
//! - STS-01: every metric declares `available`, `unavailable` or `permission_required`; a metric that is not
//!   available has no value (never 0) and carries a reason.
//! - STS-02: the health score is a weighted mean of five dimensions with the public weights and thresholds below.
//!   Its only input is the metric list: cleanup candidates, scan results or plans cannot change it.
//! - STS-03: one diagnosis line, chosen by a fixed priority; `normal` when nothing crosses a threshold.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    Unavailable,
    PermissionRequired,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metric {
    pub name: String,
    pub availability: Availability,
    pub value: Option<f64>,
    pub unit: String,
    pub reason: Option<String>,
}

impl Metric {
    /// A measured value. Non-finite values are refused and reported as unavailable instead.
    pub fn value(name: impl Into<String>, value: f64, unit: &str) -> Self {
        let name = name.into();
        if !value.is_finite() {
            return Metric::unavailable(name, unit, "the platform returned a non-finite value");
        }
        Metric { name, availability: Availability::Available, value: Some(value), unit: unit.into(), reason: None }
    }

    /// A measured value with a caveat about what it measures.
    pub fn noted(name: impl Into<String>, value: f64, unit: &str, note: &str) -> Self {
        let mut m = Metric::value(name, value, unit);
        if m.availability == Availability::Available {
            m.reason = Some(note.into());
        }
        m
    }

    pub fn unavailable(name: impl Into<String>, unit: &str, reason: impl Into<String>) -> Self {
        Metric { name: name.into(), availability: Availability::Unavailable, value: None, unit: unit.into(), reason: Some(reason.into()) }
    }

    pub fn permission_required(name: impl Into<String>, unit: &str, reason: impl Into<String>) -> Self {
        Metric { name: name.into(), availability: Availability::PermissionRequired, value: None, unit: unit.into(), reason: Some(reason.into()) }
    }
}

/// Metric names the health score and diagnosis read. Platform samplers must use these names.
pub mod names {
    pub const CPU_TOTAL: &str = "cpu.total_percent";
    pub const MEMORY_USED: &str = "memory.used_percent";
    pub const SYSTEM_FREE_PERCENT: &str = "disk.system.free_percent";
    pub const SYSTEM_FREE_BYTES: &str = "disk.system.free_bytes";
    pub const DISK_BUSY: &str = "disk.busy_percent";
    pub const DISK_FAILURE_PREDICTED: &str = "disk.system.failure_predicted";
    pub const THERMAL_MAX: &str = "thermal.max_zone_celsius";
    pub const BATTERY_PERCENT: &str = "battery.charge_percent";
    pub const BATTERY_ON_AC: &str = "battery.on_ac_power";
    pub const BATTERY_HEALTH: &str = "battery.health_percent";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dimension {
    Cpu,
    Memory,
    Disk,
    Thermal,
    Io,
}

/// Public scoring table: dimension, weight, metric, value with full score, value with zero score. Linear between
/// the two points; `full_score_at > zero_score_at` means higher is better (free space).
pub const HEALTH_TABLE: [(Dimension, u32, &str, f64, f64); 5] = [
    (Dimension::Cpu, 30, names::CPU_TOTAL, 60.0, 95.0),
    (Dimension::Memory, 25, names::MEMORY_USED, 75.0, 95.0),
    (Dimension::Disk, 20, names::SYSTEM_FREE_PERCENT, 20.0, 5.0),
    (Dimension::Thermal, 15, names::THERMAL_MAX, 70.0, 95.0),
    (Dimension::Io, 10, names::DISK_BUSY, 50.0, 95.0),
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimensionScore {
    pub dimension: Dimension,
    pub weight: u32,
    pub metric: String,
    pub value: Option<f64>,
    pub full_score_at: f64,
    pub zero_score_at: f64,
    pub score: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Health {
    /// `None` when no dimension is measurable.
    pub score: Option<u32>,
    /// Dimensions below full score, largest weighted loss first.
    pub lowered_by: Vec<Dimension>,
    pub dimensions: Vec<DimensionScore>,
}

fn find(metrics: &[Metric], name: &str) -> Option<f64> {
    metrics.iter().find(|m| m.name == name && m.availability == Availability::Available).and_then(|m| m.value)
}

/// Linear sub-score in 0..=100 between the full-score and zero-score points.
pub fn sub_score(value: f64, full: f64, zero: f64) -> u32 {
    let t = if full < zero { (value - full) / (zero - full) } else { (full - value) / (full - zero) };
    (100.0 * (1.0 - t.clamp(0.0, 1.0))).round() as u32
}

/// Health from the metric list alone. Unknown dimensions are left out and the remaining weights are rescaled,
/// so an unavailable sensor neither lowers nor raises the score.
pub fn health(metrics: &[Metric]) -> Health {
    let dimensions: Vec<DimensionScore> = HEALTH_TABLE
        .iter()
        .map(|&(dimension, weight, metric, full, zero)| {
            let value = find(metrics, metric);
            DimensionScore { dimension, weight, metric: metric.into(), value, full_score_at: full, zero_score_at: zero, score: value.map(|v| sub_score(v, full, zero)) }
        })
        .collect();
    let known: Vec<&DimensionScore> = dimensions.iter().filter(|d| d.score.is_some()).collect();
    let total_weight: u32 = known.iter().map(|d| d.weight).sum();
    let score = (total_weight > 0).then(|| {
        let sum: f64 = known.iter().map(|d| f64::from(d.weight) * f64::from(d.score.unwrap_or(0))).sum();
        (sum / f64::from(total_weight)).round() as u32
    });
    let mut lowered: Vec<(u32, Dimension)> = known.iter().filter(|d| d.score < Some(100)).map(|d| (d.weight * (100 - d.score.unwrap_or(0)), d.dimension)).collect();
    lowered.sort_by_key(|a| std::cmp::Reverse(a.0));
    Health { score, lowered_by: lowered.into_iter().map(|(_, d)| d).collect(), dimensions }
}

/// Diagnosis thresholds (public, documented in docs/platforms/windows.md).
pub mod thresholds {
    pub const HIGH_CPU_PERCENT: f64 = 85.0;
    pub const MEMORY_PRESSURE_PERCENT: f64 = 90.0;
    pub const LOW_FREE_PERCENT: f64 = 10.0;
    pub const LOW_FREE_BYTES: f64 = 10.0 * 1024.0 * 1024.0 * 1024.0;
    pub const BATTERY_LOW_PERCENT: f64 = 15.0;
    pub const BATTERY_WORN_PERCENT: f64 = 60.0;
    pub const HIGH_TEMPERATURE_CELSIUS: f64 = 90.0;
    pub const HEAVY_IO_PERCENT: f64 = 90.0;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnosis {
    pub code: String,
    pub message: String,
    /// Detail view the line opens: `status.disk`, `status.processes`, `status.memory`, `analyze`, `status.battery`,
    /// `status.thermal`; `None` for `normal`.
    pub link: Option<String>,
}

/// The busiest process by CPU, as a share of the whole machine (0..=100).
#[derive(Debug, Clone, PartialEq)]
pub struct TopProcess {
    pub name: String,
    pub cpu_percent: f64,
}

fn line(code: &str, message: String, link: &str) -> Diagnosis {
    Diagnosis { code: code.into(), message, link: Some(link.into()) }
}

fn gib(bytes: f64) -> String {
    format!("{:.1} GiB", bytes / (1024.0 * 1024.0 * 1024.0))
}

/// All diagnosis lines in priority order (STS-03): disk health, busiest process, memory pressure, low free space,
/// battery, temperature, I/O. Empty when nothing crosses a threshold.
pub fn diagnoses(metrics: &[Metric], top: Option<&TopProcess>) -> Vec<Diagnosis> {
    use thresholds::*;
    let m = |n: &str| find(metrics, n);
    let mut out = Vec::new();
    if m(names::DISK_FAILURE_PREDICTED) == Some(1.0) {
        out.push(line("disk_health_warning", "The system disk reports a predicted failure; back up now".into(), "status.disk"));
    }
    if let Some(cpu) = m(names::CPU_TOTAL).filter(|&c| c >= HIGH_CPU_PERCENT) {
        let who = top.map(|t| format!("; busiest process: {} ({:.0}%)", t.name, t.cpu_percent)).unwrap_or_default();
        out.push(line("high_cpu", format!("CPU is at {cpu:.0}%{who}"), "status.processes"));
    }
    if let Some(mem) = m(names::MEMORY_USED).filter(|&v| v >= MEMORY_PRESSURE_PERCENT) {
        out.push(line("memory_pressure", format!("Memory is {mem:.0}% used"), "status.memory"));
    }
    let free_pct = m(names::SYSTEM_FREE_PERCENT);
    let free_bytes = m(names::SYSTEM_FREE_BYTES);
    if free_pct.is_some_and(|p| p < LOW_FREE_PERCENT) || free_bytes.is_some_and(|b| b < LOW_FREE_BYTES) {
        let detail = match (free_bytes, free_pct) {
            (Some(b), Some(p)) => format!("{} ({p:.0}%)", gib(b)),
            (Some(b), None) => gib(b),
            (None, Some(p)) => format!("{p:.0}%"),
            (None, None) => String::new(),
        };
        out.push(line("low_free_space", format!("System volume free space is low: {detail}"), "analyze"));
    }
    let on_ac = m(names::BATTERY_ON_AC);
    if let Some(pct) = m(names::BATTERY_PERCENT).filter(|&p| p <= BATTERY_LOW_PERCENT && on_ac == Some(0.0)) {
        out.push(line("battery_low", format!("Battery is at {pct:.0}% and not charging"), "status.battery"));
    } else if let Some(h) = m(names::BATTERY_HEALTH).filter(|&h| h < BATTERY_WORN_PERCENT) {
        out.push(line("battery_worn", format!("Battery holds {h:.0}% of its design capacity"), "status.battery"));
    }
    if let Some(t) = m(names::THERMAL_MAX).filter(|&t| t >= HIGH_TEMPERATURE_CELSIUS) {
        out.push(line("high_temperature", format!("Thermal zone reports {t:.0} °C"), "status.thermal"));
    }
    if let Some(io) = m(names::DISK_BUSY).filter(|&v| v >= HEAVY_IO_PERCENT) {
        out.push(line("heavy_io", format!("Disks are {io:.0}% busy"), "status.disk"));
    }
    out
}

/// The single line shown (STS-03): the highest-priority problem, or `normal`.
pub fn diagnosis(lines: &[Diagnosis]) -> Diagnosis {
    lines.first().cloned().unwrap_or_else(|| Diagnosis { code: "normal".into(), message: "No metric crosses a diagnosis threshold".into(), link: None })
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatusSnapshot {
    pub sampled_at: String,
    /// True when the values are older than the time the record is emitted for (STS-06: never re-stamped).
    pub stale: bool,
    pub metrics: Vec<Metric>,
    pub health: Health,
    pub diagnoses: Vec<Diagnosis>,
}

impl StatusSnapshot {
    pub fn new(sampled_at: String, metrics: Vec<Metric>, top: Option<&TopProcess>) -> Self {
        let health = health(&metrics);
        let diagnoses = diagnoses(&metrics, top);
        StatusSnapshot { sampled_at, stale: false, metrics, health, diagnoses }
    }

    pub fn to_wire(&self) -> serde_json::Value {
        let weights: serde_json::Map<String, serde_json::Value> =
            HEALTH_TABLE.iter().map(|(d, w, ..)| (serde_json::to_value(d).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default(), (*w).into())).collect();
        let dims: Vec<serde_json::Value> = self
            .health
            .dimensions
            .iter()
            .map(|d| serde_json::json!({"dimension": d.dimension, "metric": d.metric, "value": d.value, "full_score_at": d.full_score_at, "zero_score_at": d.zero_score_at, "score": d.score}))
            .collect();
        serde_json::json!({
            "sampled_at": self.sampled_at,
            "stale": self.stale,
            "metrics": self.metrics,
            "health": {"score": self.health.score, "weights": weights, "lowered_by": self.health.lowered_by, "dimensions": dims},
            "diagnosis": diagnosis(&self.diagnoses),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> Vec<Metric> {
        vec![
            Metric::value(names::CPU_TOTAL, 20.0, "percent"),
            Metric::value(names::MEMORY_USED, 50.0, "percent"),
            Metric::value(names::SYSTEM_FREE_PERCENT, 40.0, "percent"),
            Metric::value(names::SYSTEM_FREE_BYTES, 200e9, "bytes"),
            Metric::value(names::THERMAL_MAX, 50.0, "celsius"),
            Metric::value(names::DISK_BUSY, 5.0, "percent"),
        ]
    }

    fn set(ms: &mut [Metric], name: &str, v: f64) {
        ms.iter_mut().find(|m| m.name == name).unwrap().value = Some(v);
    }

    #[test]
    fn weights_are_public_and_sum_to_100() {
        assert_eq!(HEALTH_TABLE.iter().map(|t| t.1).sum::<u32>(), 100);
        let w = StatusSnapshot::new("2026-01-01T00:00:00Z".into(), healthy(), None).to_wire();
        assert_eq!(w["health"]["weights"], serde_json::json!({"cpu": 30, "memory": 25, "disk": 20, "thermal": 15, "io": 10}));
        assert_eq!(w["health"]["dimensions"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn sub_scores_are_linear_and_clamped_in_both_directions() {
        assert_eq!(sub_score(10.0, 60.0, 95.0), 100);
        assert_eq!(sub_score(77.5, 60.0, 95.0), 50);
        assert_eq!(sub_score(100.0, 60.0, 95.0), 0);
        assert_eq!(sub_score(30.0, 20.0, 5.0), 100);
        assert_eq!(sub_score(12.5, 20.0, 5.0), 50);
        assert_eq!(sub_score(1.0, 20.0, 5.0), 0);
    }

    #[test]
    fn memory_pressure_lowers_the_score_and_names_the_dimension() {
        let base = health(&healthy());
        assert_eq!(base.score, Some(100));
        assert!(base.lowered_by.is_empty());
        let mut ms = healthy();
        set(&mut ms, names::MEMORY_USED, 93.0);
        let h = health(&ms);
        assert!(h.score.unwrap() < 100);
        assert_eq!(h.lowered_by, vec![Dimension::Memory]);
        let d = diagnoses(&ms, None);
        assert_eq!(d[0].code, "memory_pressure");
        assert!(d[0].message.contains("93%"));
        assert_eq!(d[0].link.as_deref(), Some("status.memory"));
    }

    #[test]
    fn unknown_dimensions_are_rescaled_not_zeroed() {
        let mut ms = healthy();
        ms.retain(|m| m.name != names::THERMAL_MAX);
        ms.push(Metric::unavailable(names::THERMAL_MAX, "celsius", "no sensor"));
        let h = health(&ms);
        assert_eq!(h.score, Some(100));
        let t = h.dimensions.iter().find(|d| d.dimension == Dimension::Thermal).unwrap();
        assert_eq!((t.value, t.score), (None, None));
        assert_eq!(health(&[]).score, None);
    }

    /// STS-02: the score is a pure function of the metric list; there is no input through which cleanup
    /// candidates could enter, and metrics unrelated to the five dimensions (e.g. reclaimable space) do not move it.
    #[test]
    fn score_ignores_anything_but_the_dimension_metrics() {
        let base = health(&healthy());
        let mut ms = healthy();
        ms.push(Metric::value("cleanup.candidate_count", 5000.0, "count"));
        ms.push(Metric::value("cleanup.reclaimable_bytes", 9e12, "bytes"));
        assert_eq!(health(&ms), base);
    }

    #[test]
    fn diagnosis_priority_and_links() {
        let mut ms = healthy();
        assert_eq!(diagnosis(&diagnoses(&ms, None)).code, "normal");
        assert_eq!(diagnosis(&diagnoses(&ms, None)).link, None);
        set(&mut ms, names::SYSTEM_FREE_PERCENT, 4.0);
        set(&mut ms, names::SYSTEM_FREE_BYTES, 3e9);
        set(&mut ms, names::DISK_BUSY, 99.0);
        set(&mut ms, names::THERMAL_MAX, 95.0);
        let d = diagnosis(&diagnoses(&ms, None));
        assert_eq!((d.code.as_str(), d.link.as_deref()), ("low_free_space", Some("analyze")));
        set(&mut ms, names::CPU_TOTAL, 97.0);
        let top = TopProcess { name: "busy.exe".into(), cpu_percent: 80.0 };
        let all = diagnoses(&ms, Some(&top));
        let codes: Vec<&str> = all.iter().map(|d| d.code.as_str()).collect();
        assert_eq!(codes, ["high_cpu", "low_free_space", "high_temperature", "heavy_io"]);
        assert!(all[0].message.contains("busy.exe"));
        assert_eq!(all[0].link.as_deref(), Some("status.processes"));
        ms.push(Metric::value(names::DISK_FAILURE_PREDICTED, 1.0, "boolean"));
        assert_eq!(diagnosis(&diagnoses(&ms, None)).code, "disk_health_warning");
    }

    #[test]
    fn battery_lines_need_known_power_state() {
        let mut ms = healthy();
        ms.push(Metric::value(names::BATTERY_PERCENT, 10.0, "percent"));
        assert!(diagnoses(&ms, None).is_empty(), "unknown AC state never claims 'not charging'");
        ms.push(Metric::value(names::BATTERY_ON_AC, 0.0, "boolean"));
        assert_eq!(diagnoses(&ms, None)[0].code, "battery_low");
        ms.push(Metric::value(names::BATTERY_HEALTH, 50.0, "percent"));
        set(&mut ms, names::BATTERY_ON_AC, 1.0);
        assert_eq!(diagnoses(&ms, None)[0].code, "battery_worn");
    }

    #[test]
    fn unavailable_metrics_never_carry_values() {
        let m = Metric::unavailable("fan.rpm", "rpm", "no interface");
        assert_eq!((m.value, m.availability), (None, Availability::Unavailable));
        let n = Metric::value("x", f64::NAN, "percent");
        assert_eq!(n.availability, Availability::Unavailable);
        let w = serde_json::to_value(&m).unwrap();
        assert!(w["value"].is_null());
        assert_eq!(w["availability"], "unavailable");
    }
}
