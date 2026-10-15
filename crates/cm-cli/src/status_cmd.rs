//! `status` (parity STS-01..06, tasks 12.1/12.2/12.4): one system status snapshot, or with `--watch` / `--interval` /
//! `--count` a JSONL stream of snapshots, one compact record per line.
//!
//! - STS-06: sampling runs on its own thread; every tick emits the newest finished sample, and when none finished since
//!   the previous tick the previous one again with `stale: true` and its original `sampled_at` (never re-stamped).
//! - STS-05: with alerts enabled (`status alerts --state on`, opt-in) the stream also samples per-process CPU and emits
//!   a `status_alert` record when a process stays above the threshold for the window, at most once per cooldown per
//!   program. With alerts off no process sampling happens. Only the running stream raises alerts (no background job).

use std::process::ExitCode;

use cm_core::error::ErrorCode;

use crate::args::Parsed;
use crate::fail;

/// (interval seconds, max samples) when streaming; `None` for a single snapshot.
fn stream_params(p: &Parsed) -> Result<Option<(u64, Option<u64>)>, String> {
    let interval = match p.option("interval").map(str::parse::<u64>) {
        None => None,
        Some(Ok(n)) if (1..=3600).contains(&n) => Some(n),
        Some(_) => return Err("--interval must be 1..=3600 seconds".into()),
    };
    let count = match p.option("count").map(str::parse::<u64>) {
        None => None,
        Some(Ok(n)) if (1..=100_000).contains(&n) => Some(n),
        Some(_) => return Err("--count must be 1..=100000".into()),
    };
    Ok((p.flag("watch") || interval.is_some() || count.is_some()).then(|| (interval.unwrap_or(5), count)))
}

pub fn status(req: &str, p: &Parsed) -> ExitCode {
    let stream = match stream_params(p) {
        Ok(s) => s,
        Err(msg) => return fail(req, ErrorCode::InvalidArgument, &msg, p.format),
    };
    run(req, p, stream)
}

/// `status alerts [--state on|off] [--cpu-threshold PCT] [--window-seconds N] [--cooldown-seconds N]`.
pub fn status_alerts(req: &str, p: &Parsed) -> ExitCode {
    let Some(dir) = crate::commands::store_dir(p) else {
        return fail(req, ErrorCode::InvalidArgument, "no store directory; pass --store or set CM_STORE_DIR", p.format);
    };
    let mut s = cm_app::processes::load_alerts(&dir);
    let changed = ["state", "cpu-threshold", "window-seconds", "cooldown-seconds"].iter().any(|k| p.option(k).is_some());
    match p.option("state") {
        None => {}
        Some("on") => s.enabled = true,
        Some("off") => s.enabled = false,
        Some(_) => return fail(req, ErrorCode::InvalidArgument, "--state must be on or off", p.format),
    }
    let num = |k: &str| p.option(k).map(str::parse::<f64>);
    match num("cpu-threshold") {
        Some(Ok(v)) => s.cpu_threshold_percent = v,
        Some(Err(_)) => return fail(req, ErrorCode::InvalidArgument, "--cpu-threshold must be a number", p.format),
        None => {}
    }
    for (k, slot) in [("window-seconds", &mut s.window_seconds), ("cooldown-seconds", &mut s.cooldown_seconds)] {
        match p.option(k).map(str::parse::<u32>) {
            Some(Ok(v)) => *slot = v,
            Some(Err(_)) => return fail(req, ErrorCode::InvalidArgument, &format!("--{k} must be a whole number of seconds"), p.format),
            None => {}
        }
    }
    if changed && let Err(e) = cm_app::processes::save_alerts(&dir, &s) {
        return fail(req, ErrorCode::InvalidArgument, &e, p.format);
    }
    let body = serde_json::json!({
        "enabled": s.enabled, "cpu_threshold_percent": s.cpu_threshold_percent, "window_seconds": s.window_seconds, "cooldown_seconds": s.cooldown_seconds,
        "note": "Alerts are raised only while a status stream (status --watch) or the resident component runs; off means no process sampling for alerts.",
    });
    crate::emit(&cm_core::protocol::record("status_alert_settings", req, crate::commands::fields(body)), p.format);
    ExitCode::SUCCESS
}

#[cfg(windows)]
enum Msg {
    Sample(cm_core::status::StatusSnapshot),
    Alert(cm_core::alerts::Alert),
}

#[cfg(windows)]
fn run(req: &str, p: &Parsed, stream: Option<(u64, Option<u64>)>) -> ExitCode {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use cm_core::protocol::record;

    use crate::{Format, emit};

    let snapshot = |s: &cm_core::status::StatusSnapshot| record("status_snapshot", req, crate::commands::fields(s.to_wire()));
    let Some((interval, count)) = stream else {
        emit(&snapshot(&cm_platform::windows::status::Sampler::new().sample()), p.format);
        return ExitCode::SUCCESS;
    };
    let alerts = crate::commands::store_dir(p).map(|d| cm_app::processes::load_alerts(&d)).unwrap_or_default();
    let period = Duration::from_secs(interval);
    let (tx, rx) = mpsc::channel::<Msg>();
    // Sampler thread: owns the PDH query; ends when the receiver is gone.
    std::thread::spawn(move || {
        let mut sampler = cm_platform::windows::status::Sampler::new();
        let mut engine = cm_core::alerts::AlertEngine::new(alerts);
        let mut next = Instant::now();
        loop {
            if tx.send(Msg::Sample(sampler.sample())).is_err() {
                return;
            }
            if engine.needs_sampling()
                && let Ok(procs) = cm_platform::windows::processes::list_with(Duration::from_millis(500), false)
            {
                let now = cm_core::plan::parse_utc(&cm_core::time::now_utc()).unwrap_or(0);
                let mut sample: Vec<cm_core::alerts::ProcessCpu> = procs
                    .into_iter()
                    .filter(|f| f.pid != 0)
                    .map(|f| cm_core::alerts::ProcessCpu { pid: f.pid, image_path: f.image_path.as_ref().map(|i| i.display()), name: f.name, cpu_percent: f.cpu_percent })
                    .collect();
                sample.sort_by(|a, b| b.cpu_percent.unwrap_or(0.0).total_cmp(&a.cpu_percent.unwrap_or(0.0)));
                for a in engine.observe(now, &sample) {
                    if tx.send(Msg::Alert(a)).is_err() {
                        return;
                    }
                }
            }
            next += period;
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else {
                next = now;
            }
        }
    });
    let mut ticker = cm_core::status::StreamTicker::default();
    let mut emitted = 0u64;
    // The first record waits for the first sample; nothing is invented before it.
    match rx.recv() {
        Ok(Msg::Sample(s)) => ticker.offer(s),
        Ok(Msg::Alert(_)) | Err(_) => return fail(req, ErrorCode::InternalError, "the status sampler stopped", p.format),
    }
    let alert = |a: &cm_core::alerts::Alert| emit(&record("status_alert", req, crate::commands::fields(serde_json::to_value(a).expect("alert serializes"))), Format::Json);
    // A sample due at this tick may still be finishing: wait up to this long before repeating the old one as stale.
    let grace = (period / 2).min(Duration::from_secs(3));
    let mut next = Instant::now();
    let mut first = true;
    loop {
        while let Ok(m) = rx.try_recv() {
            match m {
                Msg::Sample(s) => ticker.offer(s),
                Msg::Alert(a) => alert(&a),
            }
        }
        let deadline = Instant::now() + grace;
        while !first && !ticker.has_fresh() {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(Msg::Sample(s)) => ticker.offer(s),
                Ok(Msg::Alert(a)) => alert(&a),
                Err(_) => break,
            }
        }
        first = false;
        if let Some(s) = ticker.tick() {
            // JSONL: one compact record per line regardless of the terminal.
            emit(&snapshot(&s), Format::Json);
            emitted += 1;
        }
        if count.is_some_and(|c| emitted >= c) {
            return ExitCode::SUCCESS;
        }
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
    }
}

#[cfg(not(windows))]
fn run(req: &str, p: &Parsed, _stream: Option<(u64, Option<u64>)>) -> ExitCode {
    fail(req, ErrorCode::Unsupported, "status sampling is not implemented on this platform yet", p.format)
}
