//! Sustained high-CPU alerts (parity STS-05, task 12.4).
//!
//! - Opt-in: [`AlertSettings::default`] is disabled, and a disabled engine asks for no process sampling
//!   ([`AlertEngine::needs_sampling`]) and never alerts.
//! - A process alerts once it stayed at or above the threshold for the whole window, judged from consecutive
//!   samples; a sample below the threshold, an unknown value or a gap longer than [`MAX_GAP_SECONDS`] restarts the
//!   window. The same process (by executable path, else name) is not alerted again within the cooldown, even after a
//!   restart with a new PID.
//! - Time and samples are inputs: the engine reads no clock and no system state.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A sample older than this many seconds after the previous one breaks the "sustained" chain.
pub const MAX_GAP_SECONDS: i64 = 60;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertSettings {
    pub enabled: bool,
    /// Share of the whole machine, 1..=100.
    pub cpu_threshold_percent: f64,
    pub window_seconds: u32,
    pub cooldown_seconds: u32,
}

impl Default for AlertSettings {
    fn default() -> Self {
        AlertSettings { enabled: false, cpu_threshold_percent: 80.0, window_seconds: 120, cooldown_seconds: 1800 }
    }
}

impl AlertSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !(1.0..=100.0).contains(&self.cpu_threshold_percent) {
            return Err("cpu threshold must be 1..=100 percent".into());
        }
        if !(10..=3600).contains(&self.window_seconds) {
            return Err("window must be 10..=3600 seconds".into());
        }
        if !(60..=86_400).contains(&self.cooldown_seconds) {
            return Err("cooldown must be 60..=86400 seconds".into());
        }
        Ok(())
    }
}

/// One process in one sample.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessCpu {
    pub pid: u32,
    pub name: String,
    pub image_path: Option<String>,
    pub cpu_percent: Option<f64>,
}

impl ProcessCpu {
    /// Identity used for the window and the cooldown: the executable path, else the name (case-insensitive).
    pub fn key(&self) -> String {
        self.image_path.as_deref().unwrap_or(&self.name).to_lowercase()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alert {
    pub pid: u32,
    pub name: String,
    pub image_path: Option<String>,
    /// Lowest CPU share seen during the window.
    pub min_cpu_percent: f64,
    pub threshold_percent: f64,
    pub window_seconds: u32,
    pub since: String,
    pub raised_at: String,
    /// Detail view the notification opens.
    pub link: String,
}

#[derive(Debug, Clone)]
struct Run {
    since: i64,
    last: i64,
    min: f64,
}

#[derive(Debug, Clone, Default)]
pub struct AlertEngine {
    pub settings: AlertSettings,
    runs: BTreeMap<String, Run>,
    last_alert: BTreeMap<String, i64>,
}

impl AlertEngine {
    pub fn new(settings: AlertSettings) -> Self {
        AlertEngine { settings, runs: BTreeMap::new(), last_alert: BTreeMap::new() }
    }

    /// STS-05: when alerts are off nothing is sampled for them.
    pub fn needs_sampling(&self) -> bool {
        self.settings.enabled
    }

    /// Feed one sample taken at `now_unix`; returns the alerts raised by it.
    pub fn observe(&mut self, now_unix: i64, sample: &[ProcessCpu]) -> Vec<Alert> {
        if !self.settings.enabled {
            self.runs.clear();
            return Vec::new();
        }
        let threshold = self.settings.cpu_threshold_percent;
        let window = i64::from(self.settings.window_seconds);
        let cooldown = i64::from(self.settings.cooldown_seconds);
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for p in sample {
            let key = p.key();
            if !seen.insert(key.clone()) {
                continue; // several instances of one program: the first (busiest-sorted by caller) decides
            }
            let high = p.cpu_percent.filter(|c| *c >= threshold);
            let Some(cpu) = high else {
                self.runs.remove(&key);
                continue;
            };
            let run = match self.runs.get_mut(&key) {
                Some(r) if now_unix - r.last <= MAX_GAP_SECONDS && now_unix >= r.last => {
                    r.last = now_unix;
                    r.min = r.min.min(cpu);
                    r.clone()
                }
                _ => {
                    let r = Run { since: now_unix, last: now_unix, min: cpu };
                    self.runs.insert(key.clone(), r.clone());
                    r
                }
            };
            if now_unix - run.since < window {
                continue;
            }
            if self.last_alert.get(&key).is_some_and(|t| now_unix - t < cooldown) {
                continue;
            }
            self.last_alert.insert(key.clone(), now_unix);
            out.push(Alert {
                pid: p.pid,
                name: p.name.clone(),
                image_path: p.image_path.clone(),
                min_cpu_percent: run.min,
                threshold_percent: threshold,
                window_seconds: self.settings.window_seconds,
                since: crate::time::utc_from_unix(run.since),
                raised_at: crate::time::utc_from_unix(now_unix),
                link: format!("status.processes/{}", p.pid),
            });
        }
        self.runs.retain(|k, _| seen.contains(k));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, cpu: Option<f64>) -> ProcessCpu {
        ProcessCpu { pid, name: "busy.exe".into(), image_path: Some(r"C:\Tools\busy.exe".into()), cpu_percent: cpu }
    }

    fn on() -> AlertEngine {
        AlertEngine::new(AlertSettings { enabled: true, cpu_threshold_percent: 80.0, window_seconds: 60, cooldown_seconds: 600 })
    }

    /// STS-05: one alert after the window, none again during the cooldown (even after a restart), then again.
    #[test]
    fn sustained_alert_fires_once_per_cooldown() {
        let mut e = on();
        let mut raised = Vec::new();
        for t in (0..=300).step_by(5) {
            raised.extend(e.observe(t, &[p(10, Some(95.0))]));
        }
        assert_eq!(raised.len(), 1);
        assert_eq!(raised[0].since, crate::time::utc_from_unix(0));
        assert_eq!(raised[0].raised_at, crate::time::utc_from_unix(60));
        assert_eq!(raised[0].link, "status.processes/10");
        // Restarted under a new PID inside the cooldown: still the same program, still quiet.
        for t in (305..=640).step_by(5) {
            assert!(e.observe(t, &[p(11, Some(99.0))]).is_empty(), "t={t}");
        }
        // Cooldown over (alert at 60, cooldown 600): the next sustained window alerts again.
        let later: Vec<Alert> = (645..=700).step_by(5).flat_map(|t| e.observe(t, &[p(11, Some(99.0))])).collect();
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].pid, 11);
    }

    #[test]
    fn dips_unknowns_and_gaps_restart_the_window() {
        let mut e = on();
        for t in (0..55).step_by(5) {
            assert!(e.observe(t, &[p(1, Some(90.0))]).is_empty());
        }
        assert!(e.observe(55, &[p(1, Some(40.0))]).is_empty(), "dip");
        for t in (60..115).step_by(5) {
            assert!(e.observe(t, &[p(1, Some(90.0))]).is_empty(), "t={t}");
        }
        assert!(e.observe(115, &[p(1, None)]).is_empty(), "unknown is not high");
        assert!(e.observe(120, &[p(1, Some(90.0))]).is_empty());
        assert!(e.observe(250, &[p(1, Some(90.0))]).is_empty(), "a gap over MAX_GAP_SECONDS restarts the chain at 250");
        assert_eq!(e.observe(310, &[p(1, Some(90.0))]).len(), 1, "a 60 s gap keeps the chain: 250..310 covers the window");
        assert_eq!(e.observe(315, &[p(1, Some(90.0))]).len(), 0, "cooldown");
    }

    #[test]
    fn disabled_engine_needs_no_sampling_and_never_alerts() {
        let mut e = AlertEngine::new(AlertSettings::default());
        assert!(!AlertSettings::default().enabled, "opt-in");
        assert!(!e.needs_sampling());
        for t in 0..1000 {
            assert!(e.observe(t, &[p(1, Some(100.0))]).is_empty());
        }
        assert!(AlertSettings { cpu_threshold_percent: 0.0, ..Default::default() }.validate().is_err());
        assert!(AlertSettings::default().validate().is_ok());
    }
}
